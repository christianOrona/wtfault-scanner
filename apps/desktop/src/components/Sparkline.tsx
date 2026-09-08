// A live trace that moves the way an instrument moves.
//
// The old version redrew the whole path once per sample and rescaled the time
// axis to whatever it currently held, so every arriving reading snapped the
// entire line sideways and the vertical scale jumped whenever a new extreme
// appeared. At two samples a second that reads as a stutter, not a gauge.
//
// Three changes fix it, none of which touch how fast the vehicle is polled —
// the data rate is a property of the adapter and is not something a chart gets
// to have an opinion about:
//
//  1. **A fixed time window that slides continuously.** The x axis is always
//     "the last N seconds ending now", advanced on every animation frame rather
//     than on every sample. Between samples the trace glides left instead of
//     standing still and then jumping.
//  2. **Eased vertical scaling.** The min and max chase their targets instead
//     of teleporting, so one spike expands the axis smoothly rather than
//     yanking the whole trace down.
//  3. **A smooth curve.** Monotone cubic interpolation, which follows the data
//     without the overshoot a naive spline invents between points — it never
//     draws a peak the vehicle did not report.
//
// Everything is honest about latency: the leading dot sits at the timestamp of
// the last real sample, not at the right-hand edge, so a stalled feed visibly
// falls behind instead of pretending to keep up.

import { useEffect, useRef, useState } from "react";

/** How much history the window shows. */
const WINDOW_MS = 30_000;
/** Fraction of the remaining distance the axis closes each frame at 60fps. */
const AXIS_EASE = 0.12;

export function Sparkline({
  points,
  unit,
  height = 90,
  tone,
}: {
  points: { at: number; y: number }[];
  unit: string | null;
  height?: number;
  /** CSS colour for the trace. Defaults to the accent. */
  tone?: string;
}) {
  const [, forceFrame] = useState(0);
  const axis = useRef<{ min: number; max: number } | null>(null);
  const gradientId = useRef(`spark-${Math.random().toString(36).slice(2, 9)}`);
  const reduced = usePrefersReducedMotion();

  // One animation frame per repaint while the component is on screen. This is
  // what makes the trace slide between samples; without it the chart only
  // changes when new data arrives, which is exactly the stutter being fixed.
  useEffect(() => {
    if (reduced) return;
    let raf = 0;
    const tick = () => {
      forceFrame((n) => (n + 1) % 1_000_000);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [reduced]);

  if (points.length < 2) return null;

  const W = 400;
  const H = height;
  const PAD_L = 40;
  const PAD_R = 10;
  const PAD_T = 10;
  const PAD_B = 16;

  // The window ends now, not at the last sample, so the gap a slow adapter
  // leaves is visible rather than compressed away.
  const now = reduced ? points[points.length - 1].at : Date.now();
  const t0 = now - WINDOW_MS;

  const visible = points.filter((p) => p.at >= t0 - 1_000);
  if (visible.length < 2) return null;

  const ys = visible.map((p) => p.y);
  let targetMin = Math.min(...ys);
  let targetMax = Math.max(...ys);
  if (targetMin === targetMax) {
    // A flat series still deserves a readable band rather than a divide by zero.
    targetMin -= 1;
    targetMax += 1;
  } else {
    // A little headroom, so the trace never rides the frame edge.
    const pad = (targetMax - targetMin) * 0.12;
    targetMin -= pad;
    targetMax += pad;
  }

  if (!axis.current || reduced) {
    axis.current = { min: targetMin, max: targetMax };
  } else {
    axis.current = {
      min: ease(axis.current.min, targetMin),
      max: ease(axis.current.max, targetMax),
    };
  }
  const { min, max } = axis.current;

  const x = (at: number) => PAD_L + ((at - t0) / WINDOW_MS) * (W - PAD_L - PAD_R);
  const y = (v: number) => PAD_T + (1 - (v - min) / (max - min || 1)) * (H - PAD_T - PAD_B);

  const pts = visible.map((p) => ({ x: x(p.at), y: y(p.y) }));
  const d = monotonePath(pts);
  const area = `${d} L${pts[pts.length - 1].x.toFixed(1)},${H - PAD_B} L${pts[0].x.toFixed(1)},${H - PAD_B} Z`;
  const head = pts[pts.length - 1];
  const stroke = tone ?? "var(--accent)";

  return (
    <svg
      className="chart"
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
      height={H}
      role="img"
      aria-label={`trend over the last ${Math.round(WINDOW_MS / 1000)} seconds`}
    >
      <defs>
        <linearGradient id={gradientId.current} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={stroke} stopOpacity="0.28" />
          <stop offset="100%" stopColor={stroke} stopOpacity="0" />
        </linearGradient>
      </defs>

      <line className="grid" x1={PAD_L} y1={y(max)} x2={W - PAD_R} y2={y(max)} />
      <line className="grid" x1={PAD_L} y1={y(min)} x2={W - PAD_R} y2={y(min)} />
      <text className="chart-label" x={2} y={y(max) + 3.5}>{fmt(max)}</text>
      <text className="chart-label" x={2} y={y(min) + 3.5}>{fmt(min)}</text>
      <text className="chart-label" x={W - PAD_R} y={H - 4} textAnchor="end">
        {Math.round(WINDOW_MS / 1000)}s{unit ? ` - ${unit}` : ""}
      </text>

      <path className="area" d={area} fill={`url(#${gradientId.current})`} />
      <path className="line" d={d} stroke={stroke} vectorEffect="non-scaling-stroke" />
      {/* The head sits at the last real sample. A feed that stops moving
          visibly drifts left instead of sticking to the edge. */}
      <circle className="head" cx={head.x} cy={head.y} r={2.6} fill={stroke} />
    </svg>
  );
}

function ease(current: number, target: number): number {
  const next = current + (target - current) * AXIS_EASE;
  // Snap once the difference stops being visible, so the axis settles instead
  // of asymptotically never arriving and repainting forever.
  return Math.abs(target - next) < Math.abs(target) * 1e-4 ? target : next;
}

/**
 * Monotone cubic interpolation.
 *
 * A plain Catmull-Rom spline overshoots between points, which on a chart means
 * drawing a peak higher than anything the vehicle actually reported. This
 * variant clamps the tangents so the curve never leaves the range of the data
 * it passes through: smooth, and still only showing what was measured.
 */
function monotonePath(p: { x: number; y: number }[]): string {
  const n = p.length;
  if (n < 2) return "";
  if (n === 2) return `M${p[0].x.toFixed(1)},${p[0].y.toFixed(1)} L${p[1].x.toFixed(1)},${p[1].y.toFixed(1)}`;

  // Secant slopes between consecutive points.
  const dx: number[] = [];
  const dy: number[] = [];
  const slope: number[] = [];
  for (let i = 0; i < n - 1; i++) {
    dx[i] = p[i + 1].x - p[i].x;
    dy[i] = p[i + 1].y - p[i].y;
    slope[i] = dx[i] === 0 ? 0 : dy[i] / dx[i];
  }

  // Tangents, clamped where the data changes direction.
  const m: number[] = new Array(n);
  m[0] = slope[0];
  m[n - 1] = slope[n - 2];
  for (let i = 1; i < n - 1; i++) {
    if (slope[i - 1] * slope[i] <= 0) {
      m[i] = 0; // a local extreme: flat tangent, so no overshoot past it
    } else {
      const w1 = 2 * dx[i] + dx[i - 1];
      const w2 = dx[i] + 2 * dx[i - 1];
      m[i] = (w1 + w2) / (w1 / slope[i - 1] + w2 / slope[i]);
    }
  }

  let d = `M${p[0].x.toFixed(1)},${p[0].y.toFixed(1)}`;
  for (let i = 0; i < n - 1; i++) {
    const c1x = p[i].x + dx[i] / 3;
    const c1y = p[i].y + (m[i] * dx[i]) / 3;
    const c2x = p[i + 1].x - dx[i] / 3;
    const c2y = p[i + 1].y - (m[i + 1] * dx[i]) / 3;
    d += ` C${c1x.toFixed(1)},${c1y.toFixed(1)} ${c2x.toFixed(1)},${c2y.toFixed(1)} ${p[i + 1].x.toFixed(1)},${p[i + 1].y.toFixed(1)}`;
  }
  return d;
}

/** Honour the OS setting. An animated chart is a barrier for some people. */
function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => {
    try {
      return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    } catch {
      return false;
    }
  });
  useEffect(() => {
    let mq: MediaQueryList;
    try {
      mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    } catch {
      return;
    }
    const on = () => setReduced(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return reduced;
}

function fmt(v: number): string {
  const a = Math.abs(v);
  if (a >= 1000) return v.toFixed(0);
  if (a >= 10) return v.toFixed(1);
  return v.toFixed(2);
}
