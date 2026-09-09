//! ISO 15765-2 (ISO-TP) segmentation and reassembly.
//!
//! An ELM327 performs ISO-TP internally, so on that adapter this module is
//! mostly used to *interpret* multi-frame output when headers are on. It exists
//! as a first-class layer because the OBDLink/J2534 path in the roadmap does
//! not hide framing, and because reassembly bugs are exactly the class of bug
//! that must be caught by unit tests rather than by a truck.
//!
//! Implemented: single frame, first frame (12-bit and 32-bit escape lengths),
//! consecutive frames with wrapping sequence numbers, and flow control with
//! block size and STmin.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Protocol control information nibble values.
const PCI_SINGLE: u8 = 0x0;
const PCI_FIRST: u8 = 0x1;
const PCI_CONSECUTIVE: u8 = 0x2;
const PCI_FLOW_CONTROL: u8 = 0x3;

/// Largest payload expressible in a 12-bit first-frame length field.
pub const MAX_CLASSIC_LENGTH: usize = 0xFFF;

/// Flow status in a flow-control frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    /// Continue to send.
    ContinueToSend,
    /// Wait for a further flow-control frame.
    Wait,
    /// Overflow — the receiver cannot accept the announced length.
    Overflow,
}

impl FlowStatus {
    fn from_nibble(n: u8) -> AimResult<FlowStatus> {
        match n {
            0 => Ok(FlowStatus::ContinueToSend),
            1 => Ok(FlowStatus::Wait),
            2 => Ok(FlowStatus::Overflow),
            other => Err(AimError::new(
                ErrorCode::IsoTpError,
                format!("reserved flow status 0x{other:X}"),
            )),
        }
    }

    fn nibble(&self) -> u8 {
        match self {
            FlowStatus::ContinueToSend => 0,
            FlowStatus::Wait => 1,
            FlowStatus::Overflow => 2,
        }
    }
}

/// A decoded ISO-TP frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum IsoTpFrame {
    /// Whole message fits in one CAN frame.
    Single {
        /// Payload.
        data: Vec<u8>,
    },
    /// Start of a segmented message.
    First {
        /// Total length of the assembled message.
        total_length: usize,
        /// First chunk of payload.
        data: Vec<u8>,
    },
    /// Continuation of a segmented message.
    Consecutive {
        /// Sequence number, 0–15, wrapping.
        sequence: u8,
        /// Payload chunk.
        data: Vec<u8>,
    },
    /// Receiver's permission to continue.
    FlowControl {
        /// Continue / wait / overflow.
        status: FlowStatus,
        /// Frames the sender may send before the next flow-control frame.
        /// Zero means "send everything without further flow control".
        block_size: u8,
        /// Raw STmin byte. Use [`IsoTpFrame::st_min_duration`] to interpret it.
        st_min: u8,
    },
}

impl IsoTpFrame {
    /// Decode one CAN payload into an ISO-TP frame.
    pub fn parse(payload: &[u8]) -> AimResult<IsoTpFrame> {
        let first = *payload
            .first()
            .ok_or_else(|| AimError::new(ErrorCode::IsoTpError, "empty CAN payload"))?;
        match first >> 4 {
            PCI_SINGLE => {
                let len = (first & 0x0F) as usize;
                if len == 0 {
                    return Err(AimError::new(
                        ErrorCode::IsoTpError,
                        "single frame declares zero length",
                    ));
                }
                if payload.len() < 1 + len {
                    return Err(AimError::new(
                        ErrorCode::IsoTpError,
                        format!(
                            "single frame declares {len} bytes but only {} are present",
                            payload.len() - 1
                        ),
                    ));
                }
                Ok(IsoTpFrame::Single { data: payload[1..1 + len].to_vec() })
            }
            PCI_FIRST => {
                let short_len =
                    (((first & 0x0F) as usize) << 8) | *payload.get(1).unwrap_or(&0) as usize;
                if short_len == 0 {
                    // 32-bit escape form: 10 00 LL LL LL LL then data.
                    if payload.len() < 6 {
                        return Err(AimError::new(
                            ErrorCode::IsoTpError,
                            "escaped first frame is shorter than its length field",
                        ));
                    }
                    let total = u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]])
                        as usize;
                    if total == 0 {
                        return Err(AimError::new(
                            ErrorCode::IsoTpError,
                            "escaped first frame declares zero length",
                        ));
                    }
                    Ok(IsoTpFrame::First { total_length: total, data: payload[6..].to_vec() })
                } else {
                    if payload.len() < 2 {
                        return Err(AimError::new(ErrorCode::IsoTpError, "first frame truncated"));
                    }
                    Ok(IsoTpFrame::First { total_length: short_len, data: payload[2..].to_vec() })
                }
            }
            PCI_CONSECUTIVE => {
                Ok(IsoTpFrame::Consecutive { sequence: first & 0x0F, data: payload[1..].to_vec() })
            }
            PCI_FLOW_CONTROL => {
                if payload.len() < 3 {
                    return Err(AimError::new(
                        ErrorCode::IsoTpError,
                        "flow control frame shorter than 3 bytes",
                    ));
                }
                Ok(IsoTpFrame::FlowControl {
                    status: FlowStatus::from_nibble(first & 0x0F)?,
                    block_size: payload[1],
                    st_min: payload[2],
                })
            }
            other => Err(AimError::new(
                ErrorCode::IsoTpError,
                format!("unknown ISO-TP PCI type 0x{other:X}"),
            )),
        }
    }

    /// Encode back into a CAN payload, padded to `frame_len` with `fill`.
    pub fn encode(&self, frame_len: usize, fill: u8) -> Vec<u8> {
        let mut out = match self {
            IsoTpFrame::Single { data } => {
                let mut v = vec![(PCI_SINGLE << 4) | (data.len() as u8 & 0x0F)];
                v.extend_from_slice(data);
                v
            }
            IsoTpFrame::First { total_length, data } => {
                let mut v = if *total_length > MAX_CLASSIC_LENGTH {
                    let mut v = vec![PCI_FIRST << 4, 0x00];
                    v.extend_from_slice(&(*total_length as u32).to_be_bytes());
                    v
                } else {
                    vec![
                        (PCI_FIRST << 4) | ((total_length >> 8) as u8 & 0x0F),
                        (total_length & 0xFF) as u8,
                    ]
                };
                v.extend_from_slice(data);
                v
            }
            IsoTpFrame::Consecutive { sequence, data } => {
                let mut v = vec![(PCI_CONSECUTIVE << 4) | (sequence & 0x0F)];
                v.extend_from_slice(data);
                v
            }
            IsoTpFrame::FlowControl { status, block_size, st_min } => {
                vec![(PCI_FLOW_CONTROL << 4) | status.nibble(), *block_size, *st_min]
            }
        };
        while out.len() < frame_len {
            out.push(fill);
        }
        out
    }

    /// Interpret an STmin byte per ISO 15765-2.
    ///
    /// `0x00`–`0x7F` are milliseconds; `0xF1`–`0xF9` are 100–900 microseconds;
    /// everything else is reserved and treated as the safe maximum of 127 ms.
    pub fn st_min_duration(st_min: u8) -> Duration {
        match st_min {
            0x00..=0x7F => Duration::from_millis(st_min as u64),
            0xF1..=0xF9 => Duration::from_micros((st_min as u64 - 0xF0) * 100),
            _ => Duration::from_millis(127),
        }
    }
}

/// Result of feeding one frame into [`IsoTpReceiver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// A complete message was assembled.
    Complete(Vec<u8>),
    /// More frames are needed; nothing to send.
    NeedMore,
    /// More frames are needed and this flow-control frame must be transmitted
    /// back to the sender first.
    SendFlowControl(IsoTpFrame),
}

/// Reassembles segmented ISO-TP messages.
#[derive(Debug, Clone)]
pub struct IsoTpReceiver {
    buffer: Vec<u8>,
    expected_length: usize,
    next_sequence: u8,
    active: bool,
    block_size: u8,
    st_min: u8,
    frames_since_fc: u8,
    max_message_length: usize,
}

impl Default for IsoTpReceiver {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl IsoTpReceiver {
    /// Create a receiver that advertises `block_size` and `st_min` in the flow
    /// control frames it produces. `block_size == 0` means "no further flow
    /// control", which is what a scan tool normally wants.
    pub fn new(block_size: u8, st_min: u8) -> Self {
        IsoTpReceiver {
            buffer: Vec::new(),
            expected_length: 0,
            next_sequence: 1,
            active: false,
            block_size,
            st_min,
            frames_since_fc: 0,
            max_message_length: 8 * 1024,
        }
    }

    /// Cap the size of a message this receiver will accept. A first frame
    /// declaring more produces a flow-control `Overflow`.
    pub fn with_max_message_length(mut self, max: usize) -> Self {
        self.max_message_length = max;
        self
    }

    /// True while a segmented message is partially received.
    pub fn in_progress(&self) -> bool {
        self.active
    }

    /// Abandon any partial message.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.expected_length = 0;
        self.next_sequence = 1;
        self.active = false;
        self.frames_since_fc = 0;
    }

    /// Feed one received frame.
    pub fn feed(&mut self, frame: IsoTpFrame) -> AimResult<ReceiveOutcome> {
        match frame {
            IsoTpFrame::Single { data } => {
                self.reset();
                Ok(ReceiveOutcome::Complete(data))
            }
            IsoTpFrame::First { total_length, data } => {
                self.reset();
                if total_length > self.max_message_length {
                    return Ok(ReceiveOutcome::SendFlowControl(IsoTpFrame::FlowControl {
                        status: FlowStatus::Overflow,
                        block_size: 0,
                        st_min: 0,
                    }));
                }
                self.active = true;
                self.expected_length = total_length;
                self.buffer = data;
                self.buffer.truncate(total_length);
                self.next_sequence = 1;
                self.frames_since_fc = 0;
                if self.buffer.len() >= total_length {
                    // Degenerate but legal: everything already arrived.
                    let out = std::mem::take(&mut self.buffer);
                    self.reset();
                    return Ok(ReceiveOutcome::Complete(out));
                }
                Ok(ReceiveOutcome::SendFlowControl(IsoTpFrame::FlowControl {
                    status: FlowStatus::ContinueToSend,
                    block_size: self.block_size,
                    st_min: self.st_min,
                }))
            }
            IsoTpFrame::Consecutive { sequence, data } => {
                if !self.active {
                    return Err(AimError::new(
                        ErrorCode::IsoTpError,
                        format!("consecutive frame {sequence} with no first frame in progress"),
                    ));
                }
                if sequence != self.next_sequence {
                    let expected = self.next_sequence;
                    self.reset();
                    return Err(AimError::new(
                        ErrorCode::IsoTpError,
                        format!(
                            "out of order consecutive frame: expected {expected}, got {sequence}"
                        ),
                    ));
                }
                self.next_sequence = (self.next_sequence + 1) & 0x0F;
                let remaining = self.expected_length.saturating_sub(self.buffer.len());
                self.buffer.extend_from_slice(&data[..remaining.min(data.len())]);
                if self.buffer.len() >= self.expected_length {
                    let out = std::mem::take(&mut self.buffer);
                    self.reset();
                    return Ok(ReceiveOutcome::Complete(out));
                }
                self.frames_since_fc += 1;
                if self.block_size != 0 && self.frames_since_fc >= self.block_size {
                    self.frames_since_fc = 0;
                    return Ok(ReceiveOutcome::SendFlowControl(IsoTpFrame::FlowControl {
                        status: FlowStatus::ContinueToSend,
                        block_size: self.block_size,
                        st_min: self.st_min,
                    }));
                }
                Ok(ReceiveOutcome::NeedMore)
            }
            IsoTpFrame::FlowControl { .. } => Err(AimError::new(
                ErrorCode::IsoTpError,
                "received a flow control frame while acting as receiver",
            )),
        }
    }
}

/// Splits an outgoing message into ISO-TP frames, honouring flow control.
#[derive(Debug, Clone)]
pub struct IsoTpSender {
    payload: Vec<u8>,
    offset: usize,
    next_sequence: u8,
    frame_len: usize,
    started: bool,
    credit: Option<u8>,
    separation: Duration,
}

impl IsoTpSender {
    /// Prepare to send `payload` in frames of `frame_len` bytes (8 for
    /// classical CAN).
    pub fn new(payload: Vec<u8>, frame_len: usize) -> AimResult<Self> {
        if !(3..=64).contains(&frame_len) {
            return Err(AimError::new(
                ErrorCode::IsoTpError,
                format!("unsupported ISO-TP frame length {frame_len}"),
            ));
        }
        if payload.is_empty() {
            return Err(AimError::new(ErrorCode::IsoTpError, "empty ISO-TP payload"));
        }
        Ok(IsoTpSender {
            payload,
            offset: 0,
            next_sequence: 1,
            frame_len,
            started: false,
            credit: None,
            separation: Duration::ZERO,
        })
    }

    /// True when the message fits in a single frame.
    ///
    /// One byte of the frame is the PCI, so the payload must be strictly
    /// shorter than the frame. Written as `<` rather than `<= frame_len - 1`
    /// so a zero frame length cannot underflow.
    pub fn is_single_frame(&self) -> bool {
        self.payload.len() < self.frame_len
    }

    /// Minimum delay to observe before the next consecutive frame, as
    /// requested by the receiver's flow-control frame.
    pub fn separation_time(&self) -> Duration {
        self.separation
    }

    /// True once every byte has been handed out.
    pub fn is_complete(&self) -> bool {
        self.started && self.offset >= self.payload.len()
    }

    /// Apply a flow-control frame received from the peer.
    ///
    /// Returns `Ok(false)` for `Wait` (hold and expect another flow-control
    /// frame), `Ok(true)` for `ContinueToSend`, and an error for `Overflow`.
    pub fn apply_flow_control(&mut self, frame: &IsoTpFrame) -> AimResult<bool> {
        match frame {
            IsoTpFrame::FlowControl { status, block_size, st_min } => match status {
                FlowStatus::ContinueToSend => {
                    self.credit = if *block_size == 0 { None } else { Some(*block_size) };
                    self.separation = IsoTpFrame::st_min_duration(*st_min);
                    Ok(true)
                }
                FlowStatus::Wait => {
                    self.credit = Some(0);
                    Ok(false)
                }
                FlowStatus::Overflow => Err(AimError::new(
                    ErrorCode::IsoTpError,
                    "receiver reported buffer overflow for this message",
                )),
            },
            other => Err(AimError::new(
                ErrorCode::IsoTpError,
                format!("expected a flow control frame, got {other:?}"),
            )),
        }
    }

    /// Produce the next frame to transmit.
    ///
    /// Returns `Ok(None)` when the message is finished, or when the sender is
    /// out of block-size credit and must wait for a flow-control frame.
    pub fn next_frame(&mut self) -> AimResult<Option<IsoTpFrame>> {
        if !self.started {
            self.started = true;
            if self.is_single_frame() {
                self.offset = self.payload.len();
                return Ok(Some(IsoTpFrame::Single { data: self.payload.clone() }));
            }
            let chunk = self.frame_len - 2;
            self.offset = chunk.min(self.payload.len());
            return Ok(Some(IsoTpFrame::First {
                total_length: self.payload.len(),
                data: self.payload[..self.offset].to_vec(),
            }));
        }
        if self.offset >= self.payload.len() {
            return Ok(None);
        }
        match self.credit {
            Some(0) => return Ok(None), // blocked until the next flow control
            Some(ref mut c) => *c -= 1,
            None => {}
        }
        let chunk = self.frame_len - 1;
        let end = (self.offset + chunk).min(self.payload.len());
        let frame = IsoTpFrame::Consecutive {
            sequence: self.next_sequence,
            data: self.payload[self.offset..end].to_vec(),
        };
        self.offset = end;
        self.next_sequence = (self.next_sequence + 1) & 0x0F;
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a sender and a receiver against each other, exchanging flow
    /// control exactly as two ISO-TP peers would, and return the reassembled
    /// message plus the number of flow-control frames the receiver emitted.
    fn round_trip(payload: Vec<u8>, block_size: u8, st_min: u8) -> (Vec<u8>, usize) {
        let mut sender = IsoTpSender::new(payload, 8).unwrap();
        let mut receiver = IsoTpReceiver::new(block_size, st_min);
        let mut fc_count = 0usize;
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(guard < 10_000, "ISO-TP exchange did not terminate");
            let Some(frame) = sender.next_frame().unwrap() else {
                panic!("sender ran out of frames before the receiver completed");
            };
            // Round-trip every frame through the wire encoding so the encoder
            // and parser are exercised too.
            let wire = frame.encode(8, 0xAA);
            let decoded = IsoTpFrame::parse(&wire).unwrap();
            match receiver.feed(decoded).unwrap() {
                ReceiveOutcome::Complete(msg) => return (msg, fc_count),
                ReceiveOutcome::NeedMore => {}
                ReceiveOutcome::SendFlowControl(fc) => {
                    fc_count += 1;
                    let wire = fc.encode(8, 0x00);
                    let decoded = IsoTpFrame::parse(&wire).unwrap();
                    assert!(sender.apply_flow_control(&decoded).unwrap());
                }
            }
        }
    }

    #[test]
    fn single_frame_round_trips() {
        let payload = vec![0x41, 0x0C, 0x1A, 0xF8];
        let mut s = IsoTpSender::new(payload.clone(), 8).unwrap();
        assert!(s.is_single_frame());
        let f = s.next_frame().unwrap().unwrap();
        assert_eq!(f, IsoTpFrame::Single { data: payload.clone() });
        assert_eq!(f.encode(8, 0x00)[0], 0x04);
        assert!(s.next_frame().unwrap().is_none());
        let mut r = IsoTpReceiver::default();
        assert_eq!(r.feed(f).unwrap(), ReceiveOutcome::Complete(payload));
    }

    #[test]
    fn multi_frame_round_trips_without_block_limits() {
        // 35 bytes: FF carries 6, then five CFs of 7.
        let payload: Vec<u8> = (0..35u8).collect();
        let (out, fc) = round_trip(payload.clone(), 0, 0);
        assert_eq!(out, payload);
        assert_eq!(fc, 1, "block size 0 means exactly one flow control frame");
    }

    #[test]
    fn block_size_forces_repeated_flow_control() {
        // 62 bytes: FF (6) + 8 CFs of 7. With BS=2 the receiver must send a
        // flow control frame after every 2 consecutive frames.
        let payload: Vec<u8> = (0..62u8).collect();
        let (out, fc) = round_trip(payload.clone(), 2, 0);
        assert_eq!(out, payload);
        assert!(fc > 1, "expected repeated flow control, saw {fc}");
    }

    #[test]
    fn sender_blocks_when_out_of_credit() {
        let payload: Vec<u8> = (0..40u8).collect();
        let mut s = IsoTpSender::new(payload, 8).unwrap();
        s.next_frame().unwrap().unwrap(); // first frame
        s.apply_flow_control(&IsoTpFrame::FlowControl {
            status: FlowStatus::ContinueToSend,
            block_size: 1,
            st_min: 0,
        })
        .unwrap();
        assert!(s.next_frame().unwrap().is_some(), "one frame of credit");
        assert!(s.next_frame().unwrap().is_none(), "credit exhausted, must wait for flow control");
        s.apply_flow_control(&IsoTpFrame::FlowControl {
            status: FlowStatus::ContinueToSend,
            block_size: 0,
            st_min: 0,
        })
        .unwrap();
        assert!(s.next_frame().unwrap().is_some());
    }

    #[test]
    fn wait_status_holds_the_sender_then_releases_it() {
        let payload: Vec<u8> = (0..40u8).collect();
        let mut s = IsoTpSender::new(payload, 8).unwrap();
        s.next_frame().unwrap().unwrap();
        assert!(!s
            .apply_flow_control(&IsoTpFrame::FlowControl {
                status: FlowStatus::Wait,
                block_size: 0,
                st_min: 0
            })
            .unwrap());
        assert!(s.next_frame().unwrap().is_none());
        assert!(s
            .apply_flow_control(&IsoTpFrame::FlowControl {
                status: FlowStatus::ContinueToSend,
                block_size: 0,
                st_min: 0
            })
            .unwrap());
        assert!(s.next_frame().unwrap().is_some());
    }

    #[test]
    fn overflow_status_aborts_the_send() {
        let mut s = IsoTpSender::new(vec![0; 40], 8).unwrap();
        s.next_frame().unwrap();
        let err = s
            .apply_flow_control(&IsoTpFrame::FlowControl {
                status: FlowStatus::Overflow,
                block_size: 0,
                st_min: 0,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::IsoTpError);
    }

    #[test]
    fn oversized_messages_get_an_overflow_flow_control() {
        let mut r = IsoTpReceiver::new(0, 0).with_max_message_length(16);
        let out = r.feed(IsoTpFrame::First { total_length: 4095, data: vec![0; 6] }).unwrap();
        assert_eq!(
            out,
            ReceiveOutcome::SendFlowControl(IsoTpFrame::FlowControl {
                status: FlowStatus::Overflow,
                block_size: 0,
                st_min: 0
            })
        );
        assert!(!r.in_progress());
    }

    #[test]
    fn out_of_order_consecutive_frames_are_rejected_not_patched_over() {
        let mut r = IsoTpReceiver::new(0, 0);
        r.feed(IsoTpFrame::First { total_length: 20, data: vec![1, 2, 3, 4, 5, 6] }).unwrap();
        let err = r.feed(IsoTpFrame::Consecutive { sequence: 3, data: vec![7; 7] }).unwrap_err();
        assert_eq!(err.code, ErrorCode::IsoTpError);
        assert!(err.message.contains("expected 1"));
        assert!(!r.in_progress(), "receiver must abandon the broken message");
    }

    #[test]
    fn consecutive_frame_without_a_first_frame_is_rejected() {
        let mut r = IsoTpReceiver::new(0, 0);
        let err = r.feed(IsoTpFrame::Consecutive { sequence: 1, data: vec![0; 7] }).unwrap_err();
        assert_eq!(err.code, ErrorCode::IsoTpError);
    }

    #[test]
    fn sequence_numbers_wrap_past_fifteen() {
        // 200 bytes needs 28 consecutive frames, so the counter wraps twice.
        let payload: Vec<u8> = (0..200u8).collect();
        let (out, _) = round_trip(payload.clone(), 0, 0);
        assert_eq!(out, payload);
    }

    #[test]
    fn long_messages_use_the_32_bit_escape_length() {
        let payload = vec![0x5Au8; MAX_CLASSIC_LENGTH + 10];
        let mut s = IsoTpSender::new(payload.clone(), 8).unwrap();
        let ff = s.next_frame().unwrap().unwrap();
        let wire = ff.encode(8, 0x00);
        assert_eq!(wire[0], 0x10);
        assert_eq!(wire[1], 0x00, "escape marker");
        let parsed = IsoTpFrame::parse(&wire).unwrap();
        match parsed {
            IsoTpFrame::First { total_length, .. } => assert_eq!(total_length, payload.len()),
            other => panic!("expected a first frame, got {other:?}"),
        }
    }

    #[test]
    fn padding_bytes_are_not_leaked_into_the_message() {
        // A single frame padded with 0xAA must decode to exactly 3 bytes.
        let wire = [0x03, 0x41, 0x00, 0x01, 0xAA, 0xAA, 0xAA, 0xAA];
        assert_eq!(
            IsoTpFrame::parse(&wire).unwrap(),
            IsoTpFrame::Single { data: vec![0x41, 0x00, 0x01] }
        );
        // A final consecutive frame must be truncated to the declared length.
        let mut r = IsoTpReceiver::new(0, 0);
        r.feed(IsoTpFrame::First { total_length: 9, data: vec![1, 2, 3, 4, 5, 6] }).unwrap();
        let done = r
            .feed(IsoTpFrame::Consecutive {
                sequence: 1,
                data: vec![7, 8, 9, 0xAA, 0xAA, 0xAA, 0xAA],
            })
            .unwrap();
        assert_eq!(done, ReceiveOutcome::Complete(vec![1, 2, 3, 4, 5, 6, 7, 8, 9]));
    }

    #[test]
    fn st_min_encoding_follows_the_standard() {
        assert_eq!(IsoTpFrame::st_min_duration(0x00), Duration::ZERO);
        assert_eq!(IsoTpFrame::st_min_duration(0x7F), Duration::from_millis(127));
        assert_eq!(IsoTpFrame::st_min_duration(0xF1), Duration::from_micros(100));
        assert_eq!(IsoTpFrame::st_min_duration(0xF9), Duration::from_micros(900));
        // Reserved values fall back to the slowest safe value.
        assert_eq!(IsoTpFrame::st_min_duration(0x80), Duration::from_millis(127));
        assert_eq!(IsoTpFrame::st_min_duration(0xFA), Duration::from_millis(127));
    }

    #[test]
    fn malformed_frames_are_errors_not_guesses() {
        assert!(IsoTpFrame::parse(&[]).is_err());
        assert!(IsoTpFrame::parse(&[0x00]).is_err(), "zero-length single frame");
        assert!(IsoTpFrame::parse(&[0x07, 0x01]).is_err(), "truncated single frame");
        assert!(IsoTpFrame::parse(&[0x30, 0x00]).is_err(), "truncated flow control");
        assert!(IsoTpFrame::parse(&[0x3F, 0x00, 0x00]).is_err(), "reserved flow status");
        assert!(IsoTpFrame::parse(&[0x40, 0x00]).is_err(), "unknown PCI type");
    }
}
