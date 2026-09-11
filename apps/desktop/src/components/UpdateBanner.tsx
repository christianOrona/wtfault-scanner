// Telling somebody a new version exists.
//
// The core has been able to answer "is there a newer release?" and to download
// and launch the installer since before this file existed. Nothing ever asked
// it. A 0.2.0 install sat next to a published 0.3.0 and said nothing, because
// the whole feature was backend-only — which is a good reminder that an
// endpoint with tests is not a feature until something calls it.
//
// Deliberately quiet. This is a strip at the top, not a modal: somebody who
// opened this app is here to look at a vehicle, and a dialog in front of that
// is an interruption in service of our release notes rather than their problem.

import { useCallback, useEffect, useState } from "react";
import { api } from "../api/client";
import type { UpdateStatus } from "../api/types";
import { Spinner } from "./primitives";

/** How long after the core comes up before asking.
 *
 * Late enough that the check never competes with connecting to a vehicle,
 * which is what somebody actually opened the app to do. */
const CHECK_DELAY_MS = 4000;

export function UpdateBanner({ coreUp }: { coreUp: boolean }) {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [quitting, setQuitting] = useState(false);
  const [notes, setNotes] = useState(false);

  useEffect(() => {
    if (!coreUp) return;
    let cancelled = false;
    const t = window.setTimeout(() => {
      // A check that cannot reach GitHub is not news. It reports its own error
      // in `status.error`, and the banner stays hidden either way: somebody
      // offline does not need to be told this app failed to phone home.
      api
        .updateCheck()
        .then((s) => {
          if (!cancelled) setStatus(s);
        })
        .catch(() => {});
    }, CHECK_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [coreUp]);

  const install = useCallback(() => {
    setInstalling(true);
    setFailed(null);
    api
      .updateApply()
      // This branch used to be empty, on the theory that the window was about
      // to disappear anyway. It was not: 0.3.4 started the installer and stayed
      // open, the installer could not delete files this app was holding, and
      // the spinner said "Downloading" over a failure. The core now quits
      // itself a moment after the installer starts, so the honest thing to show
      // in that moment is that we are leaving.
      .then(() => setQuitting(true))
      .catch((e: unknown) => {
        setInstalling(false);
        setFailed(e instanceof Error ? e.message : "the download did not finish");
      });
  }, []);

  if (dismissed || !status?.update_available || !status.latest) return null;

  return (
    <div className="update-bar">
      <div className="row" style={{ gap: 10, flex: 1, minWidth: 0 }}>
        <strong>{status.latest} is available</strong>
        <span className="faint">you have {status.current}</span>
        {status.notes && (
          <button onClick={() => setNotes((n) => !n)}>
            {notes ? "hide what changed" : "what changed?"}
          </button>
        )}
        {failed && <span className="cls-bus_error">{failed}</span>}
      </div>
      <div className="row" style={{ gap: 8 }}>
        <button className="primary" onClick={install} disabled={installing}>
          {quitting ? (
            <Spinner label="Closing for the installer" />
          ) : installing ? (
            <Spinner label="Downloading" />
          ) : (
            "Update now"
          )}
        </button>
        <button onClick={() => setDismissed(true)} disabled={installing}>
          Not now
        </button>
      </div>
      {notes && status.notes && (
        // The release notes as written, not a summary. They are short, they say
        // what changed, and rewriting them here would be a second place to keep
        // the same words correct.
        <pre className="update-notes">{status.notes}</pre>
      )}
    </div>
  );
}
