// The vehicle, turning slowly, with the area a finding is in lit up inside it.
//
// This is the picture people look at first, so it is the one thing in the app
// that is allowed to be a little theatrical. What it shows is still held to
// the same rule as everything else: a typical vehicle of the chosen shape and
// a typical place for that kind of part, both said to be typical in the line
// underneath. It is not a model of the user's vehicle and never claims to be.
//
// The scene itself is in `../hologram`. This file is the frame round it: the
// shape buttons, the label that follows the lit part, and the flat outline it
// falls back to on a machine that cannot draw it.

import { useEffect, useRef, useState } from "react";
import type { Region } from "../api/types";
import { createHologram, type Hologram } from "../hologram/scene";
import type { BodyShape } from "../hologram/vehicles";
import { VehicleMap } from "./VehicleMap";

declare global {
  interface Window {
    /**
     * Set to an empty list before the page loads by `scripts/record-readme.mjs`,
     * and by nothing else. While it is there every hologram draws only when
     * stepped and adds itself to the list, so a recording can ask for the
     * picture at an exact moment and get the same one every time. The app
     * never sets it, so in the app this does nothing.
     */
    holoRecording?: Hologram[];
  }
}

const SHAPES: { id: BodyShape; label: string }[] = [
  { id: "car", label: "Car" },
  { id: "suv", label: "SUV" },
  { id: "truck", label: "Truck" },
  { id: "van", label: "Van" },
];

/** The same key the flat outline uses, so one answer serves both. */
const REMEMBERED = "aim.body-shape";

const ZONE_LABEL: Record<Exclude<Region, "unknown">, string> = {
  engine_bay: "Engine bay",
  exhaust: "Exhaust",
  fuel_system: "Fuel system",
  transmission: "Transmission",
  cabin: "Cabin",
  wheels: "At the wheels",
  electrical: "Battery and charging",
};

function rememberedShape(): BodyShape {
  try {
    const saved = localStorage.getItem(REMEMBERED);
    if (SHAPES.some((s) => s.id === saved)) return saved as BodyShape;
  } catch {
    /* Site data blocked: it simply starts on a car every time. */
  }
  return "car";
}

export default function VehicleHologram({
  region,
  partId,
  what,
  startShape,
}: {
  /** The area to light up. `unknown` renders nothing at all. */
  region: Region;
  /** The kind of part to point at, when the finding names one. */
  partId: string | null;
  /** What the label on the lit part says. */
  what: string;
  /** Start on this shape instead of the remembered one. For the preview page. */
  startShape?: BodyShape;
}) {
  const [shape, setShape] = useState<BodyShape>(() => startShape ?? rememberedShape());
  /** Set when this machine cannot draw it. The flat outline takes over. */
  const [failed, setFailed] = useState(false);
  const stage = useRef<HTMLDivElement>(null);
  const label = useRef<HTMLDivElement>(null);
  const lead = useRef<SVGPolylineElement>(null);
  const hologram = useRef<Hologram | null>(null);

  useEffect(() => {
    const host = stage.current;
    if (!host || region === "unknown") return;
    const still = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    try {
      hologram.current = createHologram(host, {
        shape,
        region,
        partId,
        still,
        manual: !!window.holoRecording,
        // Moved by hand rather than through React: this runs every frame, and
        // a render per frame to move one label would be most of the work.
        onAnchor: (at) => {
          const tag = label.current;
          const line = lead.current;
          if (!tag || !line) return;
          if (!at) {
            tag.style.opacity = "0";
            line.setAttribute("points", "");
            return;
          }
          // The label sits up and to one side of the part, on whichever side
          // has more room, joined to it by a line with one bend.
          const width = host.clientWidth;
          const left = at.x > width * 0.58;
          const elbowX = at.x + (left ? -34 : 34);
          const elbowY = Math.max(26, at.y - 46);
          const endX = elbowX + (left ? -14 : 14);
          tag.style.opacity = "1";
          tag.style.transform = left
            ? `translate(${endX}px, ${elbowY}px) translate(-100%, -50%)`
            : `translate(${endX}px, ${elbowY}px) translate(0, -50%)`;
          line.setAttribute("points", `${at.x},${at.y} ${elbowX},${elbowY} ${endX},${elbowY}`);
        },
      });
      window.holoRecording?.push(hologram.current);
    } catch {
      // No WebGL, or the driver refused. The outline says the same thing.
      setFailed(true);
    }
    return () => {
      // Out of the recording list first. In development React mounts every
      // component twice, and the first of the two would otherwise stay listed
      // after it was thrown away, to be stepped with no canvas left to draw on.
      const listed = window.holoRecording?.indexOf(hologram.current!) ?? -1;
      if (listed >= 0) window.holoRecording!.splice(listed, 1);
      hologram.current?.dispose();
      hologram.current = null;
    };
    // Built once per mount. Shape and focus changes go to the live scene
    // below, so the car does not blink out and start again when one changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    hologram.current?.setShape(shape);
  }, [shape]);

  useEffect(() => {
    hologram.current?.setFocus(region, partId);
  }, [region, partId]);

  function choose(next: BodyShape) {
    setShape(next);
    try {
      localStorage.setItem(REMEMBERED, next);
    } catch {
      /* Not being able to remember it does not stop it being used now. */
    }
  }

  // Nothing the standard places is nothing to show.
  if (region === "unknown") return null;
  if (failed) return <VehicleMap region={region} what={what} generic={!!partId} />;

  return (
    <div className="holo">
      <div
        className="holo-stage"
        ref={stage}
        role="img"
        aria-label={`A typical ${shape} with the ${ZONE_LABEL[region]} lit up, where ${what} is`}
      >
        <svg className="holo-lead" aria-hidden="true">
          <polyline ref={lead} />
        </svg>
        <div className="holo-label" ref={label} aria-hidden="true">
          {what}
        </div>
        <div className="holo-zone" aria-hidden="true">
          <span className="holo-dot" />
          {ZONE_LABEL[region]}
        </div>
        <div className="holo-shapes">
          {SHAPES.map((s) => (
            <button
              key={s.id}
              className={s.id === shape ? "on" : ""}
              onClick={() => choose(s.id)}
              aria-pressed={s.id === shape}
            >
              {s.label}
            </button>
          ))}
        </div>
        <div className="holo-hint" aria-hidden="true">drag to turn</div>
      </div>
      <div className="holo-caption">
        <strong>{ZONE_LABEL[region]}</strong>
        <span className="faint">
          {" "}— a typical {shape === "suv" ? "SUV" : shape} and a typical place for this, not a
          model of yours. Where it is and what it looks like vary by make and model.
        </span>
      </div>
    </div>
  );
}
