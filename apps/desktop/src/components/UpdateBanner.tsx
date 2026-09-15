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
import type { DownloadState, UpdateStatus } from "../api/types";
import { highlightsOf } from "../releaseNotes";
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
  const [download, setDownload] = useState<DownloadState | null>(null);

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

  // Nothing is fetched until somebody asks for it.
  //
  // This used to start downloading the installer the moment the check found
  // one, so that pressing Install cost a restart rather than a wait. It also
  // meant the application moved a five-megabyte executable onto the machine
  // that nobody had asked for, which is a different thing from telling
  // somebody a version exists — and the code signing policy this project
  // publishes says the application sends and fetches nothing unasked beyond
  // that one check. So the check still runs, and the download is a button.
  //
  // What is read here is local: whether a download started earlier in this
  // run is already under way or finished. It touches no network.
  useEffect(() => {
    if (!status?.update_available) return;
    let cancelled = false;
    api
      .updateDownloadStatus()
      .then((d) => {
        if (!cancelled) setDownload(d);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [status?.update_available]);

  // While a download is running, follow it.
  const downloading = download?.stage === "downloading";
  useEffect(() => {
    if (!downloading) return;
    let cancelled = false;
    let timer = 0;
    const poll = async () => {
      try {
        const d = await api.updateDownloadStatus();
        if (cancelled) return;
        setDownload(d);
        if (d.stage === "downloading") timer = window.setTimeout(poll, 400);
      } catch {
        /* A download that cannot be asked about is reported by its own state. */
      }
    };
    timer = window.setTimeout(poll, 400);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [downloading]);

  const startDownload = useCallback(() => {
    setFailed(null);
    api
      .updateDownload()
      .then(setDownload)
      .catch((e: unknown) => {
        setFailed(e instanceof Error ? e.message : "the download could not start");
      });
  }, []);

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

  const ready = download?.stage === "ready";
  const percent =
    download?.stage === "downloading" && download.total
      ? Math.min(99, Math.round((download.downloaded / download.total) * 100))
      : null;
  // A download that failed is worth saying out loud: without this the banner
  // would sit on a spinner forever and look like a slow network.
  const downloadError = download?.stage === "failed" ? download.error : null;

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
        {ready && <span className="faint">downloaded — installs when you say so</span>}
        {status.size != null && !ready && !downloading && (
          <span className="faint">{Math.max(1, Math.round(status.size / 1_000_000))} MB</span>
        )}
        {(failed || downloadError) && (
          <span className="cls-bus_error">{failed ?? downloadError}</span>
        )}
      </div>
      <div className="row" style={{ gap: 8 }}>
        {/* Two presses, each one a decision. Download fetches the installer
            and leaves the app usable while it does; Install and restart is
            offered only once the bytes are on disk, because the restart is the
            part that interrupts somebody and they choose when. */}
        {ready || installing || quitting ? (
          <button className="primary" onClick={install} disabled={installing}>
            {quitting ? (
              <Spinner label="Restarting" />
            ) : installing ? (
              <Spinner label="Installing" />
            ) : (
              "Install and restart"
            )}
          </button>
        ) : downloading ? (
          <button className="primary" disabled>
            <Spinner label={percent === null ? "Starting the download" : `Downloading ${percent}%`} />
          </button>
        ) : (
          <button className="primary" onClick={startDownload}>
            {downloadError ? "Try the download again" : "Download"}
          </button>
        )}
        <button onClick={() => setDismissed(true)} disabled={installing}>
          Not now
        </button>
      </div>
      {notes && status.notes && (
        // Still the release notes as published — not a second set of words to
        // keep correct — but read for their shape rather than printed raw. The
        // <pre> this replaced showed asterisks, headings and column-78 line
        // breaks in monospace, which made every release look like log output.
        <div className="update-notes">
          {highlightsOf(status.notes).map((h) => (
            <div className="update-note" key={h.headline}>
              <span className="update-note-mark" aria-hidden="true" />
              <div className="update-note-text">
                <div className="update-note-head">{h.headline}</div>
                {h.body && <div className="update-note-body">{h.body}</div>}
              </div>
            </div>
          ))}
          {status.url && (
            <a
              className="update-note-more"
              href={status.url}
              target="_blank"
              rel="noopener noreferrer"
              title={status.url}
            >
              All of it on GitHub
            </a>
          )}
        </div>
      )}
    </div>
  );
}
