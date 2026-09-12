// What this application has established about the vehicle in front of it.
//
// Distinct from every other screen here, which show what the vehicle *said*.
// This shows what was concluded from it, including the conclusions that were
// negative — and those come first.
//
// # Why ruled-out leads
//
// "This does not work on this vehicle" is the expensive finding. Nothing
// suggests it in advance, it costs a session on a real truck to learn, and it
// is the one most easily lost. A 2019 F-250 spent two evenings establishing
// that a documented mirror-fold mapping does nothing on it; a panel that
// buried that under a list of successes would invite somebody to spend a third.
//
// # Why it lives on this screen
//
// "Settings on the car" is where somebody asks what this vehicle can be made to
// do. What it has already been shown *not* to do belongs in the same place, or
// the screen keeps offering things that have been tried and did not work.

import { useCallback, useEffect, useState } from "react";
import { api } from "../api/client";
import type { VehicleFinding } from "../api/types";

/** Order: what was ruled out, then what holds, then what was merely seen. */
const RANK: Record<string, number> = { ruled_out: 0, established: 1, observed: 2 };

const LABEL: Record<string, string> = {
  ruled_out: "ruled out",
  established: "established",
  observed: "observed",
};

export function VehicleKnowledgePanel() {
  const [findings, setFindings] = useState<VehicleFinding[] | null>(null);
  const [expanded, setExpanded] = useState(false);

  const load = useCallback(async () => {
    try {
      const k = await api.vehicleKnowledge();
      setFindings(k.findings ?? []);
    } catch {
      // Nothing connected, or nothing known. Either way this panel has nothing
      // to say and says nothing, rather than reporting its own failure on a
      // screen about the vehicle.
      setFindings(null);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (!findings || findings.length === 0) return null;

  const sorted = [...findings].sort(
    (a, b) => (RANK[a.outcome] ?? 3) - (RANK[b.outcome] ?? 3),
  );
  const ruledOut = sorted.filter((f) => f.outcome === "ruled_out").length;
  const shown = expanded ? sorted : sorted.slice(0, 4);

  return (
    <div className="card" style={{ marginBottom: 12 }}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <strong>What has been established about this vehicle</strong>
        <span className="faint" style={{ fontSize: 11 }}>
          {findings.length} finding{findings.length === 1 ? "" : "s"}
          {ruledOut > 0 && `, ${ruledOut} ruled out`}
        </span>
      </div>
      <p className="faint" style={{ fontSize: 11, margin: "6px 0 10px" }}>
        Measured on this vehicle rather than shipped with the app. It is kept
        against the VIN, so another vehicle gets its own.
      </p>

      <div className="knowledge-list">
        {shown.map((f) => (
          <div className="knowledge-item" key={f.subject}>
            <span className={`knowledge-mark ${f.outcome}`}>{LABEL[f.outcome] ?? f.outcome}</span>
            <div style={{ minWidth: 0 }}>
              <div className="knowledge-claim">{f.claim}</div>
              {/* The evidence, always. A claim nobody can attribute is one
                  somebody will repeat without being able to say why. */}
              <div className="knowledge-evidence">{f.evidence}</div>
            </div>
          </div>
        ))}
      </div>

      {sorted.length > 4 && (
        <button
          style={{ marginTop: 8 }}
          onClick={() => setExpanded((e) => !e)}
        >
          {expanded ? "show less" : `show all ${sorted.length}`}
        </button>
      )}
    </div>
  );
}
