// What the car is being asked, right now, where nobody can miss it.
//
// # Why this exists
//
// A full scan of a two-network truck sweeps 255 diagnostic addresses and takes
// upward of a minute. Every screen that could have said so put its own small
// spinner somewhere inside itself — the header, a button, a pane the user had
// since navigated away from — and the reasonable conclusion from a window with
// no visible spinner and no new results is that the application has hung. It
// was reported as exactly that: "is the full scan stuck?"
//
// So: one strip, under the header, on every screen, for any operation from any
// pane or from the assistant. It comes from the server's own answer to "what is
// the adapter doing", not from whichever component happened to start the work,
// which is the only way it can be right when the work was started somewhere
// else.
//
// # Why it counts up rather than across
//
// There is no progress to report. The adapter knows how many addresses are
// left; it cannot know how many of them will answer, and an address that
// answers costs a tenth of what a silent one does. A bar filling at a rate
// nobody can honour is a worse lie than a number counting up, and the number is
// what actually answers the question being asked — this has been going for
// forty seconds, not two hundred.

import type { AdapterBusy } from "../api/types";

/** Roughly how long each operation runs, measured on a 2019 F-250.
 *
 * Used only to set expectations in words. Nothing branches on it, and an
 * operation missing from here simply gets no estimate rather than a made-up
 * one. */
const USUALLY: Record<string, string> = {
  "a full scan of every module": "about a minute on a vehicle scanned before, longer on the first pass",
  "a scan for modules": "a few seconds",
  "a comparison against the factory configuration": "a minute or two — it reads every module",
  "a capability probe of one module": "a few seconds",
  "reading the as-built configuration": "a few seconds",
};

export function BusyBar({ busy }: { busy: AdapterBusy | null | undefined }) {
  if (!busy) return null;
  const usually = USUALLY[busy.doing];
  return (
    <div className="busybar" role="status" aria-live="polite">
      <span className="spin" />
      {/* The server names the operation in the lower case of a sentence, because
          that is where it is also used ("busy with a full scan of every
          module"). Here it is the heading, so the first letter is raised in CSS
          rather than by keeping two spellings of the same string in sync. */}
      <strong className="busybar-what">{busy.doing}</strong>
      <span className="mono">{formatElapsed(busy.seconds)}</span>
      {usually && <span className="faint">usually {usually}</span>}
      <span className="spacer" />
      {/* Said plainly, because the alternative reading of a frozen button is
          that the application is broken. */}
      <span className="faint">
        The car answers one question at a time, so everything else waits for this.
      </span>
    </div>
  );
}

function formatElapsed(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}m ${String(s).padStart(2, "0")}s`;
}
