// What a kind of part looks like, drawn.
//
// # What these are
//
// Line drawings of a KIND of part: an oxygen sensor, a catalytic converter.
// Each shows the two or three things that let somebody recognise one under a
// vehicle, which is nearly always a shape and whatever is attached to it: the
// wire, the hose, the row of them. They are not drawings of the part on any
// particular vehicle, and the caption beside one says so.
//
// The descriptions come from the core (`vehicle-profiles/generic-obd/parts.yaml`).
// Only the drawings live here, keyed by the same ids, because a drawing is
// presentation and a description is knowledge. A part with no drawing is shown
// in words alone, which is a gap you can see rather than a wrong picture.
//
// # Why they are this plain
//
// SVG primitives in the app's own colours, for the same reasons as the vehicle
// outline: no dependency, no asset to license, and a diff a person can read. A
// photograph would be of one manufacturer's part and would be read as "yours
// looks like this".

import type { ReactNode } from "react";

/** One drawing, on a 120 x 80 canvas. `a` marks the part itself where the
 *  drawing also shows what it is fitted to. */
const SKETCHES: Record<string, ReactNode> = {
  // A plug in a pipe, with its wire. The wire is how you find one.
  oxygen_sensor: (
    <>
      <rect x={8} y={34} width={20} height={12} rx={6} className="psk-a" />
      <line x1={14} y1={36} x2={14} y2={44} className="psk-l" />
      <line x1={19} y1={36} x2={19} y2={44} className="psk-l" />
      <rect x={28} y={31} width={14} height={18} className="psk-a" />
      <path d="M31,31 V49 M35,31 V49 M39,31 V49" className="psk-l" />
      <path d="M42,28 L50,28 L55,40 L50,52 L42,52 Z" className="psk-a" />
      <rect x={55} y={33} width={20} height={14} rx={3} className="psk-a" />
      <path d="M75,40 C92,40 84,20 100,20" className="psk-l" />
      <rect x={100} y={14} width={13} height={12} rx={2} className="psk-l" />
    </>
  ),

  // The first big canister in the exhaust, with a sensor either side.
  catalytic_converter: (
    <>
      <path d="M4,36 H22 M4,44 H22 M98,36 H116 M98,44 H116" className="psk-l" />
      <path d="M22,36 L34,24 H86 L98,36 V44 L86,56 H34 L22,44 Z" className="psk-a" />
      <path d="M42,26 V54 M50,26 V54 M58,26 V54 M66,26 V54 M74,26 V54" className="psk-l psk-faint" />
      <path d="M14,36 V28 M106,36 V28" className="psk-l" />
      <rect x={11} y={22} width={6} height={6} rx={1} className="psk-l" />
      <rect x={103} y={22} width={6} height={6} rx={1} className="psk-l" />
    </>
  ),

  // Longer, and with two thin pipes running off to a pressure sensor.
  particulate_filter: (
    <>
      <path d="M2,42 H14 M2,50 H14 M106,42 H118 M106,50 H118" className="psk-l" />
      <path d="M14,42 L24,32 H96 L106,42 V50 L96,60 H24 L14,50 Z" className="psk-a" />
      <path d="M34,32 V60 M86,32 V60" className="psk-l psk-faint" />
      <path d="M30,32 C30,16 44,14 56,14 M90,32 C90,20 80,18 68,18" className="psk-l" />
      <rect x={56} y={9} width={12} height={12} rx={2} className="psk-l" />
    </>
  ),

  // A round valve with a pipe from the exhaust and a plug on top.
  egr_valve: (
    <>
      <circle cx={52} cy={30} r={18} className="psk-a" />
      <circle cx={52} cy={30} r={7} className="psk-l" />
      <rect x={44} y={48} width={16} height={12} className="psk-a" />
      <path d="M36,60 H68 V68 H36 Z" className="psk-l" />
      <path d="M68,64 C90,64 92,50 110,50 M68,68 C92,68 96,56 110,56" className="psk-l" />
      <rect x={46} y={5} width={12} height={8} rx={2} className="psk-l" />
    </>
  ),

  // A block pushed into the side of the big air pipe.
  mass_air_flow_sensor: (
    <>
      <ellipse cx={20} cy={48} rx={7} ry={18} className="psk-l" />
      <path d="M20,30 H96 M20,66 H96" className="psk-l" />
      <path d="M96,30 A7,18 0 0 1 96,66" className="psk-l" />
      <rect x={46} y={16} width={24} height={20} rx={3} className="psk-a" />
      <rect x={52} y={36} width={12} height={14} className="psk-a" />
      <rect x={70} y={20} width={12} height={9} rx={2} className="psk-l" />
    </>
  ),

  // A ring with a flap in it and a motor on the side.
  throttle_body: (
    <>
      <rect x={18} y={12} width={56} height={56} rx={8} className="psk-a" />
      <circle cx={46} cy={40} r={20} className="psk-l" />
      <ellipse cx={46} cy={40} rx={6} ry={19} className="psk-l" transform="rotate(20 46 40)" />
      <rect x={74} y={24} width={28} height={32} rx={4} className="psk-l" />
      <rect x={102} y={34} width={10} height={12} rx={2} className="psk-l" />
    </>
  ),

  // A small plug with two pins.
  coolant_temp_sensor: (
    <>
      <rect x={18} y={35} width={22} height={10} rx={5} className="psk-a" />
      <rect x={40} y={31} width={14} height={18} className="psk-a" />
      <path d="M43,31 V49 M47,31 V49 M51,31 V49" className="psk-l" />
      <path d="M54,27 L62,27 L67,40 L62,53 L54,53 Z" className="psk-a" />
      <rect x={67} y={30} width={26} height={20} rx={3} className="psk-l" />
      <path d="M76,36 V44 M84,36 V44" className="psk-l" />
    </>
  ),

  // A disc with a spring, which lives inside a hose housing.
  thermostat: (
    <>
      <ellipse cx={60} cy={30} rx={38} ry={8} className="psk-a" />
      <path d="M40,24 C40,8 80,8 80,24" className="psk-l" />
      <path d="M44,38 L76,44 L44,50 L76,56 L44,62 L76,68" className="psk-l" />
      <path d="M40,37 V70 H80 V37" className="psk-l psk-faint" />
      <circle cx={60} cy={16} r={3} className="psk-l" />
    </>
  ),

  // A block on top with a plug, and a long boot down onto the spark plug.
  ignition_coil: (
    <>
      <rect x={38} y={8} width={34} height={20} rx={3} className="psk-a" />
      <rect x={72} y={12} width={14} height={10} rx={2} className="psk-l" />
      <circle cx={46} cy={18} r={3} className="psk-l" />
      <path d="M48,28 V60 C48,68 62,68 62,60 V28" className="psk-a" />
      <path d="M52,68 V74 H58 V68" className="psk-l" />
      <path d="M48,38 H62 M48,46 H62" className="psk-l psk-faint" />
    </>
  ),

  // A finger-sized cylinder with a plug on its side and a spray at the tip.
  fuel_injector: (
    <>
      <rect x={50} y={6} width={16} height={10} rx={2} className="psk-a" />
      <path d="M50,12 H66 M50,16 H66" className="psk-l" />
      <rect x={46} y={16} width={24} height={30} rx={3} className="psk-a" />
      <path d="M70,22 L88,14 L92,24 L70,32" className="psk-l" />
      <path d="M52,46 H64 L61,60 H55 Z" className="psk-a" />
      <path d="M58,62 V72 M54,62 L48,72 M62,62 L68,72" className="psk-l psk-faint" />
    </>
  ),

  // A stub with one bolt through a tab, poked into the engine.
  position_sensor: (
    <>
      <rect x={10} y={34} width={36} height={14} rx={3} className="psk-a" />
      <path d="M46,30 H62 V52 H46 Z" className="psk-a" />
      <path d="M50,30 C50,14 70,14 70,30 V34 H62" className="psk-l" />
      <circle cx={60} cy={24} r={4} className="psk-l" />
      <rect x={62} y={34} width={30} height={14} rx={3} className="psk-l" />
      <rect x={92} y={36} width={12} height={10} rx={2} className="psk-l" />
    </>
  ),

  // Two snail shells back to back.
  turbocharger: (
    <>
      <circle cx={36} cy={42} r={24} className="psk-a" />
      <circle cx={36} cy={42} r={8} className="psk-l" />
      <path d="M36,18 H60 V28" className="psk-l" />
      <rect x={60} y={36} width={14} height={12} className="psk-l" />
      <circle cx={90} cy={42} r={18} className="psk-a" />
      <circle cx={90} cy={42} r={6} className="psk-l" />
      <path d="M90,60 V72 H104" className="psk-l" />
    </>
  ),

  // A shoebox of charcoal with hoses, and a small valve on one of them.
  evap_system: (
    <>
      <rect x={14} y={30} width={56} height={36} rx={4} className="psk-a" />
      <path d="M22,30 V66 M30,30 V66" className="psk-l psk-faint" />
      <path d="M26,30 V20 C26,12 40,12 48,12 M50,30 V22 H84" className="psk-l" />
      <rect x={84} y={15} width={16} height={14} rx={3} className="psk-a" />
      <path d="M100,22 H114 M92,15 V8" className="psk-l" />
      <path d="M70,54 H88 V70" className="psk-l" />
    </>
  ),

  // The cap you take off to fill up.
  fuel_cap: (
    <>
      <circle cx={46} cy={40} r={26} className="psk-a" />
      <rect x={24} y={34} width={44} height={12} rx={4} className="psk-l" />
      <circle cx={46} cy={40} r={32} className="psk-l psk-faint" />
      <path d="M72,40 C90,40 90,62 104,62" className="psk-l" />
      <circle cx={107} cy={62} r={3} className="psk-l" />
    </>
  ),

  // A float on an arm, hanging from the plate on top of the tank.
  fuel_level_sender: (
    <>
      <ellipse cx={40} cy={12} rx={24} ry={6} className="psk-a" />
      <rect x={32} y={18} width={16} height={46} rx={3} className="psk-a" />
      <path d="M48,34 L92,56" className="psk-l" />
      <rect x={88} y={50} width={22} height={12} rx={6} className="psk-l" />
      <path d="M6,62 C30,58 60,66 114,62" className="psk-l psk-faint" />
    </>
  ),

  // A heavy box with two posts and two thick cables.
  battery: (
    <>
      <rect x={18} y={26} width={84} height={44} rx={4} className="psk-a" />
      <rect x={28} y={18} width={12} height={8} className="psk-l" />
      <rect x={80} y={18} width={12} height={8} className="psk-l" />
      <path d="M30,40 H38 M34,36 V44 M82,40 H90" className="psk-l" />
      <path d="M34,18 C34,6 14,10 6,6 M86,18 C86,6 106,10 114,6" className="psk-l" />
    </>
  ),
};

/** Whether there is a drawing for this id. */
export function hasSketch(id: string | null | undefined): boolean {
  return !!id && id in SKETCHES;
}

/**
 * The drawing for one kind of part, or nothing when there is none.
 *
 * `name` is what a screen reader is told: a drawing with no text alternative
 * would be the one part of this the person who most needs it cannot use.
 */
export function PartSketch({ id, name }: { id: string; name: string }) {
  const drawing = SKETCHES[id];
  if (!drawing) return null;
  return (
    <svg viewBox="0 0 120 80" className="psk" role="img" aria-label={`Drawing of a typical ${name}`}>
      {drawing}
    </svg>
  );
}
