// The four vehicle bodies, as numbers.
//
// # What this is
//
// A car, an SUV, a pickup and a van, each described by a handful of curves:
// how high the bonnet, the roof and the floor are along the length, and how
// wide the body is. `ringAt` turns those into the body's cross-section at any
// point from nose to tail, and everything drawn is made of those rings: the
// wireframe, the surface that hides the far side, the bright creases, and the
// band of light that sweeps down the body.
//
// # Why they are built from numbers rather than loaded from a file
//
// A model file is somebody's model of one particular vehicle, with a licence,
// and the picture would be read as "this is your car". These are typical
// proportions and say so. They also carry what a file would not: where the
// engine, the exhaust, the tank and the battery sit, in the same coordinates
// as the body, so the part of the vehicle a fault is in can be lit up inside
// the shell it belongs to.
//
// Lengths are in metres. The nose is at +x, up is +y, the vehicle's left is -z.

import * as THREE from "three";
import type { Region } from "../api/types";

export type BodyShape = "car" | "suv" | "truck" | "van";

/** A value at a point along the length: `[t, value]`, t = 0 at the nose. */
type Pt = [number, number];

export interface BodySpec {
  label: string;
  length: number;
  halfWidth: number;
  wheelbase: number;
  frontOverhang: number;
  wheelRadius: number;
  tireWidth: number;
  /** Top of the lower body: bonnet, waistline, boot lid or bed rail. */
  deck: Pt[];
  /** The roof, from the base of the windscreen to the base of the back glass. */
  roof: Pt[];
  /** The underside. */
  bottom: Pt[];
  /** Half-width as a fraction of `halfWidth`, seen from above. */
  plan: Pt[];
  /** A pickup's open bed. */
  bed?: { from: number; to: number; floor: number };
  /** Where the roof pillars stand. */
  pillars: number[];
  /** Where the door edges are. */
  doors: number[];
  /** The top and bottom edges of the windscreen and the back glass. */
  glassEdges: number[];
  /** Which wheels the engine drives, which decides where the gearbox is. */
  drive: "front" | "rear";
  engine: { length: number; width: number; height: number };
  /** Centre of the fuel tank as t, and which side its filler is on. */
  tank: { t: number; length: number; width: number; z: number };
}

export const BODIES: Record<BodyShape, BodySpec> = {
  car: {
    label: "Car",
    length: 4.8,
    halfWidth: 0.92,
    wheelbase: 2.85,
    frontOverhang: 0.92,
    wheelRadius: 0.33,
    tireWidth: 0.23,
    deck: [[0, 0.6], [0.02, 0.67], [0.09, 0.74], [0.3, 0.86], [0.5, 0.9], [0.78, 0.94], [0.86, 0.99], [0.965, 0.97], [1, 0.8]],
    roof: [[0.3, 0.86], [0.435, 1.36], [0.53, 1.43], [0.66, 1.4], [0.86, 0.99]],
    bottom: [[0, 0.27], [0.04, 0.2], [0.12, 0.16], [0.88, 0.17], [0.96, 0.22], [1, 0.3]],
    plan: [[0, 0.8], [0.03, 0.91], [0.11, 0.98], [0.3, 1], [0.82, 1], [0.94, 0.96], [1, 0.86]],
    pillars: [0.435, 0.56, 0.7],
    doors: [0.31, 0.5, 0.69],
    glassEdges: [0.31, 0.44, 0.67, 0.85],
    drive: "front",
    engine: { length: 0.55, width: 0.78, height: 0.5 },
    tank: { t: 0.69, length: 0.7, width: 0.95, z: 0 },
  },
  suv: {
    label: "SUV",
    length: 4.75,
    halfWidth: 0.96,
    wheelbase: 2.8,
    frontOverhang: 0.9,
    wheelRadius: 0.39,
    tireWidth: 0.26,
    deck: [[0, 0.86], [0.015, 0.93], [0.09, 0.99], [0.27, 1.06], [0.5, 1.1], [0.9, 1.14], [0.985, 1.12], [1, 1.02]],
    roof: [[0.27, 1.06], [0.385, 1.64], [0.5, 1.72], [0.86, 1.7], [0.965, 1.62], [1, 1.38]],
    bottom: [[0, 0.36], [0.04, 0.28], [0.12, 0.24], [0.88, 0.25], [0.96, 0.3], [1, 0.4]],
    plan: [[0, 0.88], [0.03, 0.96], [0.1, 1], [0.95, 1], [1, 0.94]],
    pillars: [0.385, 0.53, 0.7, 0.88],
    doors: [0.28, 0.48, 0.68],
    glassEdges: [0.28, 0.39, 0.955, 0.995],
    drive: "rear",
    engine: { length: 0.72, width: 0.66, height: 0.6 },
    tank: { t: 0.7, length: 0.75, width: 1.0, z: 0 },
  },
  truck: {
    label: "Truck",
    length: 5.9,
    halfWidth: 1.0,
    wheelbase: 3.7,
    frontOverhang: 0.98,
    wheelRadius: 0.43,
    tireWidth: 0.29,
    deck: [[0, 1.04], [0.012, 1.1], [0.075, 1.15], [0.225, 1.2], [0.45, 1.23], [0.595, 1.25], [0.62, 1.3], [0.985, 1.3], [1, 1.22]],
    roof: [[0.225, 1.2], [0.315, 1.78], [0.4, 1.87], [0.55, 1.86], [0.588, 1.8], [0.602, 1.25]],
    bottom: [[0, 0.44], [0.03, 0.36], [0.1, 0.3], [0.9, 0.32], [0.97, 0.38], [1, 0.48]],
    plan: [[0, 0.93], [0.02, 0.98], [0.07, 1], [0.985, 1], [1, 0.97]],
    bed: { from: 0.615, to: 0.99, floor: 0.82 },
    pillars: [0.315, 0.44, 0.585],
    doors: [0.235, 0.42, 0.59],
    glassEdges: [0.235, 0.32, 0.575, 0.6],
    drive: "rear",
    engine: { length: 0.95, width: 0.66, height: 0.7 },
    tank: { t: 0.5, length: 1.15, width: 0.5, z: -0.42 },
  },
  van: {
    label: "Van",
    length: 5.2,
    halfWidth: 1.0,
    wheelbase: 3.25,
    frontOverhang: 0.95,
    wheelRadius: 0.36,
    tireWidth: 0.24,
    deck: [[0, 0.74], [0.015, 0.83], [0.085, 0.94], [0.165, 1.0], [0.5, 1.05], [0.95, 1.07], [1, 0.96]],
    roof: [[0.165, 1.0], [0.27, 1.78], [0.34, 1.93], [0.96, 1.93], [0.99, 1.86], [1, 1.6]],
    bottom: [[0, 0.32], [0.04, 0.24], [0.12, 0.2], [0.9, 0.21], [0.97, 0.26], [1, 0.34]],
    plan: [[0, 0.88], [0.03, 0.96], [0.1, 1], [0.97, 1], [1, 0.95]],
    pillars: [0.27, 0.41, 0.63, 0.93],
    doors: [0.175, 0.39, 0.62],
    glassEdges: [0.175, 0.275, 0.975, 0.998],
    drive: "front",
    engine: { length: 0.55, width: 0.8, height: 0.5 },
    tank: { t: 0.66, length: 0.8, width: 1.0, z: 0 },
  },
};

/** How many points one cross-section has between its control points. */
const SUB = 3;
/** Control points round one cross-section: 10 up one side, 8 back down the other. */
const CONTROLS = 18;
/** Points round one sampled cross-section. */
export const RING = CONTROLS * SUB;

function at(points: Pt[], t: number): number {
  if (t <= points[0][0]) return points[0][1];
  for (let i = 1; i < points.length; i++) {
    const [t1, v1] = points[i];
    if (t <= t1) {
      const [t0, v0] = points[i - 1];
      return v0 + ((v1 - v0) * (t - t0)) / Math.max(1e-6, t1 - t0);
    }
  }
  return points[points.length - 1][1];
}

/** `at`, with corners eased: a body panel does not turn on a knife edge. */
function eased(points: Pt[], t: number, spread = 0.012): number {
  let sum = 0;
  for (const k of [-2, -1, 0, 1, 2]) sum += at(points, Math.min(1, Math.max(0, t + k * spread * 0.5)));
  return sum / 5;
}

function clamp01(v: number): number {
  return Math.min(1, Math.max(0, v));
}

/** A body with its measurements worked out once. */
export class Body {
  readonly spec: BodySpec;
  /** Front and rear axle positions along x. */
  readonly axles: [number, number];

  constructor(readonly shape: BodyShape) {
    this.spec = BODIES[shape];
    const front = this.spec.length / 2 - this.spec.frontOverhang;
    this.axles = [front, front - this.spec.wheelbase];
  }

  /** Where along x a point `t` of the way from nose to tail is. */
  x(t: number): number {
    return this.spec.length / 2 - t * this.spec.length;
  }

  /** How far along the body a position x is. */
  t(x: number): number {
    return (this.spec.length / 2 - x) / this.spec.length;
  }

  deck(t: number): number {
    return eased(this.spec.deck, t);
  }

  roof(t: number): number {
    const { roof } = this.spec;
    const deck = this.deck(t);
    if (t < roof[0][0] || t > roof[roof.length - 1][0]) return deck;
    return Math.max(deck, eased(roof, t, 0.006));
  }

  bottom(t: number): number {
    return eased(this.spec.bottom, t);
  }

  halfWidth(t: number): number {
    return this.spec.halfWidth * eased(this.spec.plan, t);
  }

  /** How much of a cabin there is here: 0 over the bonnet, 1 under the roof. */
  cabin(t: number): number {
    return clamp01((this.roof(t) - this.deck(t)) / 0.22);
  }

  /** The height a wheel arch cuts the body side to at x, or 0 away from a wheel. */
  private arch(x: number): number {
    const r = this.spec.wheelRadius;
    const opening = r + 0.075;
    for (const axle of this.axles) {
      const dx = Math.abs(x - axle);
      if (dx < opening) return r + Math.sqrt(opening * opening - dx * dx);
    }
    return 0;
  }

  /**
   * The ten points that shape one side of the cross-section at `t`, from the
   * middle of the floor up the side and over to the middle of the roof.
   */
  controls(t: number): THREE.Vector2[] {
    const w = this.halfWidth(t);
    const yb = this.bottom(t);
    const yd = this.deck(t);
    const yr = this.roof(t);
    const h = yd - yb;
    const c = this.cabin(t);
    const crown = 0.045;

    const lower: [number, number][] = [
      [0, yb],
      [0.78 * w, yb],
      [0.955 * w, yb + 0.12 * h],
      [w, yb + 0.5 * h],
      [0.985 * w, yd - 0.1 * h],
      [0.93 * w, yd],
    ];
    const arch = this.arch(this.x(t));
    for (const k of [1, 2, 3]) lower[k][1] = Math.max(lower[k][1], Math.min(arch, yd - 0.06));

    // Over the bonnet the top is a shallow crown; under the roof it is glass
    // leaning in to a roof rail. Between the two is the windscreen.
    const hood: [number, number][] = [
      [0.8 * w, yd + 0.5 * crown],
      [0.54 * w, yd + 0.85 * crown],
      [0.27 * w, yd + 0.97 * crown],
      [0, yd + crown],
    ];
    const glass: [number, number][] = [
      [0.895 * w, yd + 0.035],
      [0.75 * w, yr - 0.075],
      [0.5 * w, yr - 0.008],
      [0, yr],
    ];
    let upper = hood.map(([z, y], i): [number, number] => [
      z + (glass[i][0] - z) * c,
      y + (glass[i][1] - y) * c,
    ]);

    const bed = this.spec.bed;
    if (bed && t > bed.from && t < bed.to) {
      // An open box: the rail, straight down the inside, and across the floor.
      upper = [[0.9 * w, yd], [0.86 * w, bed.floor], [0.45 * w, bed.floor], [0, bed.floor]];
    }
    return [...lower, ...upper].map(([z, y]) => new THREE.Vector2(z, y));
  }

  /** The cross-section at `t`, as `RING` points round the body. */
  ringAt(t: number): THREE.Vector3[] {
    const half = this.controls(t);
    const x = this.x(t);
    const loop: THREE.Vector3[] = half.map((p) => new THREE.Vector3(x, p.y, p.x));
    for (let k = 8; k >= 1; k--) loop.push(new THREE.Vector3(x, half[k].y, -half[k].x));
    return new THREE.CatmullRomCurve3(loop, true, "catmullrom", 0.28).getPoints(RING).slice(0, RING);
  }
}

/** Which ring point a control point becomes. `mirror` picks the other side. */
export function ringIndex(control: number, mirror = false): number {
  return ((mirror ? CONTROLS - control : control) % CONTROLS) * SUB;
}

/** The points of ring `ring` from control `from` to control `to`, on one side. */
export function ringSpan(ring: THREE.Vector3[], from: number, to: number, mirror = false): THREE.Vector3[] {
  const out: THREE.Vector3[] = [];
  for (let i = from * SUB; i <= to * SUB; i++) {
    out.push(ring[(mirror ? RING - i : i) % RING]);
  }
  return out;
}

/** What is inside the shell, by the area of the vehicle it belongs to. */
export interface Internals {
  /** Line geometry for each area, in the body's coordinates. */
  groups: Record<Exclude<Region, "unknown">, THREE.BufferGeometry[]>;
  /** Where to point for an area, and for each kind of part within one. */
  anchors: Record<string, THREE.Vector3>;
}

function edges(geometry: THREE.BufferGeometry, at: THREE.Vector3, rotateToX = false): THREE.BufferGeometry {
  if (rotateToX) geometry.rotateZ(Math.PI / 2);
  geometry.translate(at.x, at.y, at.z);
  const out = new THREE.EdgesGeometry(geometry, 20);
  geometry.dispose();
  return out;
}

function box(size: [number, number, number], at: THREE.Vector3): THREE.BufferGeometry {
  return edges(new THREE.BoxGeometry(...size, 2, 2, 2), at);
}

/** A can lying along the vehicle: a catalytic converter, a silencer. */
function can(radius: number, length: number, at: THREE.Vector3, taper = 1): THREE.BufferGeometry {
  return edges(new THREE.CylinderGeometry(radius * taper, radius, length, 14, 3), at, true);
}

function polyline(points: THREE.Vector3[]): THREE.BufferGeometry {
  const pairs: THREE.Vector3[] = [];
  for (let i = 1; i < points.length; i++) pairs.push(points[i - 1], points[i]);
  return new THREE.BufferGeometry().setFromPoints(pairs);
}

/** A pipe: rings along a path, and four lines joining them. */
function pipe(path: THREE.Vector3[], radius: number): THREE.BufferGeometry {
  const curve = new THREE.CatmullRomCurve3(path, false, "catmullrom", 0.2);
  const tube = new THREE.TubeGeometry(curve, path.length * 10, radius, 6, false);
  const out = new THREE.EdgesGeometry(tube, 28);
  tube.dispose();
  return out;
}

function circle(radius: number, at: THREE.Vector3, segments = 28): THREE.BufferGeometry {
  const points: THREE.Vector3[] = [];
  for (let i = 0; i <= segments; i++) {
    const a = (i / segments) * Math.PI * 2;
    points.push(new THREE.Vector3(at.x + Math.cos(a) * radius, at.y + Math.sin(a) * radius, at.z));
  }
  return polyline(points);
}

/**
 * The engine, gearbox, exhaust, tank, battery, seats and brakes, placed where
 * a vehicle of this layout typically has them.
 *
 * Typical, not measured: a front engine, the exhaust down one side of the
 * floor, the tank ahead of the rear axle. That is what the caption beside the
 * picture says, and it is why a part is pointed at on this and never on a
 * drawing of the user's own vehicle.
 */
export function buildInternals(body: Body): Internals {
  const { spec } = body;
  const [front, rear] = body.axles;
  const v = (x: number, y: number, z: number) => new THREE.Vector3(x, y, z);
  const floor = body.bottom(0.5) + 0.07;
  const cowl = body.x(spec.roof[0][0]);
  const hood = body.deck(body.t(front));
  const longitudinal = spec.drive === "rear";

  // Engine, over the front axle.
  const e = spec.engine;
  const engine = v(front + 0.06, floor + 0.13 + e.height / 2, 0);
  const engineTop = engine.y + e.height / 2;
  const engineBay: THREE.BufferGeometry[] = [
    box([e.length, e.height, e.width], engine),
    // The intake: an air box at the front corner and a pipe to the engine.
    box([0.3, 0.2, 0.26], v(front + 0.5, engineTop - 0.05, -spec.halfWidth * 0.55)),
    pipe(
      [v(front + 0.4, engineTop - 0.05, -spec.halfWidth * 0.5), v(front + 0.2, engineTop + 0.05, -0.2), v(engine.x, engineTop + 0.06, 0)],
      0.05,
    ),
    // The radiator, behind the grille.
    box([0.06, e.height * 0.9, spec.halfWidth * 1.3], v(body.x(0.035), engine.y + 0.02, 0)),
  ];

  // Gearbox. Behind the engine on a rear-drive layout, beside it otherwise.
  const transmission: THREE.BufferGeometry[] = [];
  let gearbox: THREE.Vector3;
  if (longitudinal) {
    const length = 0.8;
    gearbox = v(engine.x - e.length / 2 - length / 2, floor + 0.2, 0);
    transmission.push(can(0.24, length, gearbox, 0.55));
    // Propshaft to the rear axle, and the differential on it.
    const shaftStart = gearbox.x - length / 2;
    transmission.push(pipe([v(shaftStart, floor + 0.14, 0), v(rear + 0.2, body.spec.wheelRadius, 0)], 0.035));
    transmission.push(edges(new THREE.SphereGeometry(0.17, 8, 6), v(rear, spec.wheelRadius, 0)));
    transmission.push(polyline([v(rear, spec.wheelRadius, -spec.halfWidth * 0.8), v(rear, spec.wheelRadius, spec.halfWidth * 0.8)]));
  } else {
    gearbox = v(engine.x - 0.02, floor + 0.2, e.width / 2 + 0.16);
    transmission.push(box([0.42, 0.36, 0.3], gearbox));
    transmission.push(polyline([v(front, spec.wheelRadius, -spec.halfWidth * 0.8), v(front, spec.wheelRadius, spec.halfWidth * 0.8)]));
  }

  // Exhaust: down from the engine, back under the floor, out at the tail.
  const side = longitudinal ? 0.3 : -0.26;
  const run = floor - 0.02;
  const catX = cowl - 0.5;
  const midX = Math.max(rear + 0.75, catX - 1.0 - (spec.wheelbase - 2.8) * 0.5);
  const tail = body.x(1) + 0.06;
  const silencerX = longitudinal && spec.bed ? (midX + rear) / 2 - 0.2 : rear - 0.42;
  const exhaustPath = [
    v(engine.x - e.length / 2 + 0.05, engine.y - 0.05, side * 0.8),
    v(cowl + 0.05, run + 0.12, side),
    v(catX, run, side),
    v(midX, run, side),
    v(rear, run + 0.04, side * 1.1),
    v(silencerX < rear ? silencerX : rear - 0.42, run + 0.02, side * 1.5),
    v(tail, run + 0.04, side * 1.9),
  ];
  const exhaust: THREE.BufferGeometry[] = [
    pipe(exhaustPath, 0.04),
    can(0.11, 0.42, v(catX, run, side)),
    can(0.12, 0.6, v(midX, run, side)),
    can(0.14, 0.62, v(silencerX, run + 0.02, silencerX < rear ? side * 1.5 : side * 1.05)),
  ];

  // Fuel tank, and the filler pipe up to the cap on the body side.
  const tankX = body.x(spec.tank.t);
  const tank = v(tankX, floor + 0.12, spec.tank.z);
  const capT = body.t(rear) + 0.02 - (spec.bed ? 0.09 : 0);
  const capY = body.deck(capT) - 0.2;
  const cap = v(body.x(capT) + spec.wheelRadius + 0.2, capY, -body.halfWidth(capT));
  const canister = v(tankX - spec.tank.length / 2 - 0.22, floor + 0.14, spec.tank.z + 0.3);
  const fuelSystem: THREE.BufferGeometry[] = [
    box([spec.tank.length, 0.26, spec.tank.width], tank),
    pipe([v(tank.x - 0.1, tank.y + 0.12, tank.z - 0.2), v(cap.x - 0.05, capY - 0.25, cap.z + 0.2), cap], 0.035),
    circle(0.07, cap, 16),
    box([0.24, 0.18, 0.2], canister),
    // The fuel line forward to the engine.
    polyline([v(tank.x + spec.tank.length / 2, floor + 0.08, tank.z * 0.5 - 0.15), v(cowl, floor + 0.08, -0.15), v(engine.x, engineTop - 0.08, -0.12)]),
  ];

  // Battery and alternator.
  const battery = v(front + 0.42, hood - 0.3, spec.halfWidth * 0.52);
  const alternator = v(engine.x + e.length / 2 - 0.08, engine.y + 0.08, e.width / 2 + 0.02);
  const electrical: THREE.BufferGeometry[] = [
    box([0.3, 0.2, 0.19], battery),
    polyline([v(battery.x - 0.1, battery.y + 0.1, battery.z), v(battery.x - 0.1, battery.y + 0.16, battery.z)]),
    polyline([v(battery.x + 0.1, battery.y + 0.1, battery.z), v(battery.x + 0.1, battery.y + 0.16, battery.z)]),
    edges(new THREE.CylinderGeometry(0.085, 0.085, 0.14, 10, 1), alternator.clone()),
    polyline([battery, v(battery.x - 0.2, battery.y - 0.1, battery.z - 0.2), alternator]),
  ];

  // The cabin: seats and a steering wheel.
  const cabin: THREE.BufferGeometry[] = [];
  const roofStart = spec.roof[0][0];
  const roofEnd = spec.bed ? spec.bed.from : spec.roof[spec.roof.length - 1][0];
  const rows = spec.bed ? [0.3, 0.62] : [0.28, 0.6];
  const seatY = body.deck(0.5) - 0.42;
  for (const row of rows) {
    const t = roofStart + (roofEnd - roofStart) * row + 0.02;
    for (const z of [-0.4, 0.4]) {
      const sx = body.x(t);
      cabin.push(box([0.48, 0.12, 0.46], v(sx, seatY, z * spec.halfWidth * 1.05)));
      cabin.push(box([0.1, 0.56, 0.44], v(sx - 0.26, seatY + 0.33, z * spec.halfWidth * 1.05)));
    }
  }
  const wheelPos = v(body.x(roofStart + (roofEnd - roofStart) * 0.16), seatY + 0.42, -0.4 * spec.halfWidth);
  const steering = new THREE.TorusGeometry(0.17, 0.014, 4, 18);
  steering.rotateY(Math.PI / 2);
  steering.rotateZ(-0.35);
  cabin.push(edges(steering, wheelPos));

  // Brakes: a disc inside each wheel.
  const wheels: THREE.BufferGeometry[] = [];
  const track = spec.halfWidth - spec.tireWidth / 2 - 0.03;
  for (const axle of body.axles) {
    for (const z of [-track, track]) {
      const hub = v(axle, spec.wheelRadius, z + (z < 0 ? 0.07 : -0.07));
      // The rim itself as well, so all four corners light up, not four small discs.
      wheels.push(circle(spec.wheelRadius * 1.02, v(axle, spec.wheelRadius, z + (z < 0 ? -1 : 1) * spec.tireWidth * 0.5), 36));
      wheels.push(circle(spec.wheelRadius * 0.62, hub));
      wheels.push(circle(spec.wheelRadius * 0.2, hub, 12));
      wheels.push(box([0.16, 0.12, 0.05], v(axle + spec.wheelRadius * 0.45, spec.wheelRadius + 0.1, hub.z)));
    }
  }

  const anchors: Record<string, THREE.Vector3> = {
    engine_bay: v(engine.x, engineTop, 0),
    exhaust: v(midX, run, side),
    fuel_system: v(tank.x, tank.y + 0.13, tank.z),
    transmission: gearbox.clone(),
    cabin: v(body.x((roofStart + roofEnd) / 2), seatY + 0.5, 0),
    wheels: v(front, spec.wheelRadius, -track),
    electrical: battery.clone(),

    oxygen_sensor: v(cowl + 0.05, run + 0.12, side),
    nox_sensor: v(midX - 0.42, run, side),
    catalytic_converter: v(catX, run, side),
    particulate_filter: v(midX, run, side),
    egr_valve: v(engine.x - e.length / 2 + 0.08, engineTop, side * 0.9),
    mass_air_flow_sensor: v(front + 0.36, engineTop - 0.02, -spec.halfWidth * 0.46),
    throttle_body: v(engine.x + 0.05, engineTop + 0.06, -0.08),
    coolant_temp_sensor: v(engine.x + e.length / 2, engineTop - 0.06, 0.12),
    thermostat: v(engine.x + e.length / 2 + 0.02, engineTop - 0.1, -0.12),
    ignition_coil: v(engine.x, engineTop, 0.1),
    fuel_injector: v(engine.x, engineTop - 0.06, -0.14),
    position_sensor: v(engine.x + e.length / 2, engine.y - e.height / 2 + 0.08, 0),
    turbocharger: v(engine.x - e.length / 4, engine.y + 0.08, -side * 1.2),
    evap_system: canister.clone(),
    fuel_cap: cap.clone(),
    fuel_level_sender: v(tank.x, tank.y + 0.13, tank.z),
    battery: battery.clone(),
  };

  return {
    groups: { engine_bay: engineBay, exhaust, fuel_system: fuelSystem, transmission, cabin, wheels, electrical },
    anchors,
  };
}
