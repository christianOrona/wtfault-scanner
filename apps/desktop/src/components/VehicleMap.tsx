// Roughly where a finding lives on a vehicle of this shape.
//
// # The constraint is the design
//
// This project has part locations for no vehicle, and there is no source it
// can ship that does. A particulate filter sits somewhere different on an
// F-250 than on a Golf, and two F-250s of different years differ. So a
// rendering with a glowing dot at a precise point would be inventing a fact —
// the exact thing the rest of this application refuses to do.
//
// What survives that is zones. A catalyst fault is in the exhaust on every
// vehicle ever built, because that is where a catalyst has to be. A wheel
// speed sensor is at a wheel. An airbag is in the cabin. Those follow from
// what the part is *for*, not from how one manufacturer laid the vehicle out,
// so they are true everywhere — and they are genuinely useful to somebody who
// does not know what a DPF is and now knows to look under the vehicle rather
// than under the bonnet.
//
// The label says exactly that, every time it is shown.
//
// # Why the person picks the shape
//
// Body style is not something this app can read. It is not in the VIN in any
// way it can decode without a licensed database, and guessing it from a make
// would be the same invention in a different coat. So the person says, once,
// and it is remembered. Until they do, there is no diagram — which is the
// honest state, not a broken one.
//
// # Why SVG
//
// It scales, it themes from the same variables as everything else, it diffs in
// review, and it adds no dependency. A 3D model would be a large binary asset
// and a renderer to draw a picture that says exactly this much.

import { useState } from "react";
import type { Region } from "../api/types";

/** The shapes offered. Coarse on purpose: these are the silhouettes that
 *  actually change where things sit, and a longer list would imply a precision
 *  the drawing does not have. */
const SHAPES = [
  { id: "truck", label: "Pickup truck" },
  { id: "suv", label: "SUV / 4x4" },
  { id: "car", label: "Car" },
  { id: "van", label: "Van" },
] as const;

export type BodyShape = (typeof SHAPES)[number]["id"];

const REMEMBERED = "aim.body-shape";

/** What each zone is called on screen, matching `aim_decoders::Region`. */
const ZONE_LABEL: Record<Exclude<Region, "unknown">, string> = {
  engine_bay: "Engine bay",
  exhaust: "Exhaust",
  fuel_system: "Fuel system",
  transmission: "Transmission",
  cabin: "Cabin",
  wheels: "At the wheels",
  electrical: "Battery and charging",
};

/**
 * The zones, as fractions of a 200×80 side elevation with the nose at the left.
 *
 * Fractions rather than a traced outline per body style: the differences
 * between a truck and a saloon that matter here are proportions — where the
 * cabin starts, how much of the length is engine — and those are expressible
 * as numbers. Tracing four accurate silhouettes would look better and say
 * nothing more.
 */
const ZONES: Record<
  Exclude<Region, "unknown">,
  { x: number; y: number; w: number; h: number }
> = {
  engine_bay: { x: 6, y: 30, w: 46, h: 26 },
  cabin: { x: 56, y: 18, w: 62, h: 34 },
  transmission: { x: 60, y: 52, w: 38, h: 14 },
  fuel_system: { x: 120, y: 44, w: 42, h: 20 },
  // Under the vehicle and above the ground line, which is where an exhaust
  // actually runs. The first version crossed the ground and read as a part
  // buried in the road.
  exhaust: { x: 56, y: 59, w: 122, h: 6 },
  electrical: { x: 8, y: 20, w: 22, h: 12 },
  wheels: { x: 0, y: 0, w: 0, h: 0 }, // drawn as the two wheels, not a box
};

/** Outlines, one per shape. Only the roofline really differs. */
const OUTLINE: Record<BodyShape, string> = {
  //            nose      bonnet     screen      roof        back        tail
  truck: "M4,58 L4,40 L40,38 L58,20 L104,20 L110,34 L190,34 L190,58 Z",
  suv: "M4,58 L4,40 L34,38 L52,16 L150,16 L176,36 L190,40 L190,58 Z",
  car: "M4,58 L6,42 L40,40 L70,18 L130,18 L166,40 L190,44 L190,58 Z",
  van: "M4,58 L4,38 L26,36 L40,12 L180,12 L190,20 L190,58 Z",
};

/** Where the wheels sit, per shape. */
const WHEELS: Record<BodyShape, [number, number]> = {
  truck: [40, 158],
  suv: [42, 156],
  car: [44, 152],
  van: [40, 162],
};

export function VehicleMap({
  region,
  what,
}: {
  /** The zone to highlight. `unknown` renders nothing at all. */
  region: Region;
  /** What is being pointed at, for the caption. */
  what: string;
}) {
  const [shape, setShape] = useState<BodyShape | null>(() => {
    try {
      const saved = localStorage.getItem(REMEMBERED);
      return SHAPES.some((s) => s.id === saved) ? (saved as BodyShape) : null;
    } catch {
      // A browser with site data blocked. The picker simply appears every
      // time, which is a mild annoyance rather than a broken screen.
      return null;
    }
  });

  function choose(s: BodyShape) {
    setShape(s);
    try {
      localStorage.setItem(REMEMBERED, s);
    } catch {
      /* Not being able to remember it does not stop it being used now. */
    }
  }

  // Nothing the standard places is nothing to show. A diagram with no
  // highlight invites somebody to read the absence as "it is fine".
  if (region === "unknown") return null;

  if (!shape) {
    return (
      <div className="vmap-ask">
        <div className="faint">
          What shape is your vehicle? The app cannot read that from the VIN, and it changes
          roughly where things sit.
        </div>
        <div className="row" style={{ gap: 6, marginTop: 8 }}>
          {SHAPES.map((s) => (
            <button key={s.id} className="mini" onClick={() => choose(s.id)}>
              {s.label}
            </button>
          ))}
        </div>
      </div>
    );
  }

  const zone = ZONES[region];
  const [front, rear] = WHEELS[shape];

  return (
    <div className="vmap">
      <svg viewBox="0 0 200 80" className="vmap-svg" role="img"
           aria-label={`${what} is roughly in the ${ZONE_LABEL[region]} of a vehicle of this shape`}>
        <path d={OUTLINE[shape]} className="vmap-body" />

        {/* Over the outline, and translucent so the outline still reads
            through it. Underneath, the body's own stroke would cut the
            highlight into pieces wherever the two crossed. */}
        {region === "wheels" ? (
          <>
            <circle cx={front} cy={58} r={13} className="vmap-zone" />
            <circle cx={rear} cy={58} r={13} className="vmap-zone" />
          </>
        ) : (
          <rect x={zone.x} y={zone.y} width={zone.w} height={zone.h} rx={4} className="vmap-zone" />
        )}

        <circle cx={front} cy={58} r={9} className="vmap-wheel" />
        <circle cx={rear} cy={58} r={9} className="vmap-wheel" />
        <line x1={0} y1={68} x2={200} y2={68} className="vmap-ground" />
      </svg>

      {/* The caption is not decoration. It is the difference between a useful
          diagram and a claim this app cannot support. */}
      <div className="vmap-caption">
        <strong>{ZONE_LABEL[region]}</strong>
        <span className="faint">
          {" "}— roughly where {what} sits on a vehicle of this shape. This is not your
          vehicle's layout: nobody has measured where that part is on your particular
          vehicle, and this app will not pretend otherwise.
        </span>
        <button className="mini" style={{ marginLeft: 8 }} onClick={() => setShape(null)}>
          Wrong shape?
        </button>
      </div>
    </div>
  );
}
