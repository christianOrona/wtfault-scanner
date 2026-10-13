// What to look for and roughly where, for one finding.
//
// The question after "what is wrong?" is "what is that, and where is it?". This
// answers both as far as it honestly can: a drawing and a description of that
// KIND of part, and the area of a vehicle it is in. Both are true of every
// vehicle that has the part, because they follow from what the part is for.
// Neither is this vehicle's own layout, which nobody has measured, and one
// line under the picture says so.
//
// Everything shown comes from the core: `placement` is read from the trouble
// code or self-test the finding rests on, by the published ranges. Nothing here
// decides where anything is, and a finding the standard does not place renders
// nothing at all.

import { Suspense, lazy } from "react";
import type { Part, Region } from "../api/types";
import { PartSketch, hasSketch } from "./PartSketch";

// Loaded when first needed: the 3D library is most of a megabyte, and a scan
// with nothing to place never asks for it.
const VehicleHologram = lazy(() => import("./VehicleHologram"));

export interface Placed {
  region?: Region | null;
  part?: Part | null;
}

/** Whether there is anything to show for this. */
export function isPlaced(p: Placed | null | undefined): boolean {
  return !!p && (!!p.part || (!!p.region && p.region !== "unknown"));
}

export function WhereItIs({
  placed,
  what,
}: {
  placed: Placed | null | undefined;
  /** What the finding calls it, used when no kind of part is named. */
  what: string;
}) {
  if (!placed || !isPlaced(placed)) return null;
  const { part } = placed;
  const region: Region = part?.zone ?? placed.region ?? "unknown";
  const sketch = part ? (part.sketch ?? part.id) : null;

  return (
    <div className="where">
      {/* The vehicle first and full width: it is what somebody looks at. What
          the part is and how to find it sits under it, in words. */}
      <Suspense fallback={<div className="holo holo-loading">Drawing the vehicle…</div>}>
        <VehicleHologram region={region} partId={part?.id ?? null} what={part?.name ?? what} />
      </Suspense>
      {part && (
        <div className="where-part">
          {hasSketch(sketch) && <PartSketch id={sketch!} name={part.name} />}
          <div>
            <div className="where-name">{part.name}</div>
            <div className="where-text">{part.looks_like}</div>
            <div className="where-text">
              <span className="faint">Where to look: </span>
              {part.find_it}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
