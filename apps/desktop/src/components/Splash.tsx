// The first two seconds.
//
// Not decoration. The desktop shell starts the HTTP core and the window at the
// same time, and the window usually wins the race — so the first thing anyone
// saw was an empty grey frame with no name on it while the core came up. That
// reads as a broken app for exactly as long as it takes to start, which on a
// cold Windows boot is a few seconds.
//
// So this fills that gap with the thing that ought to be there anyway: the
// name, the artwork, and a line saying what is happening. It leaves as soon as
// the core answers, and it will not outstay a slow start — after a few seconds
// it says so, because a splash that never goes away is worse than no splash.

import { useCallback, useEffect, useState } from "react";
import splash from "../assets/splash.png";

/** How long the artwork stays up even when the core is ready sooner.
 *
 * The core now starts fast enough that the splash was gone before anyone
 * registered it. This is a floor, not a delay: it never holds a *slow* start
 * open any longer than it already would be.
 *
 * A constant rather than a number buried in a component, because it is the kind
 * of value someone will want to change without going looking for it. Eight
 * seconds is a long time to add to every launch forever, which is exactly why
 * a click or a key skips it. */
const MIN_VISIBLE_MS = 8000;

export function Splash({ ready, stalled }: { ready: boolean; stalled: boolean }) {
  const [gone, setGone] = useState(false);
  const [minElapsed, setMinElapsed] = useState(false);
  const [skipped, setSkipped] = useState(false);

  useEffect(() => {
    const t = window.setTimeout(() => setMinElapsed(true), MIN_VISIBLE_MS);
    return () => window.clearTimeout(t);
  }, []);

  // Leaving needs both: the core has answered, and it has been up long enough
  // to see. Skipping overrides the floor and never the core - dismissing the
  // artwork must not reveal an application that is not running yet.
  const leaving = ready && (minElapsed || skipped);

  const skip = useCallback(() => setSkipped(true), []);

  useEffect(() => {
    if (!leaving) return;
    // Fade out rather than cut, and unmount after the transition so the image
    // is not sitting in the tree forever.
    const t = window.setTimeout(() => setGone(true), 420);
    return () => window.clearTimeout(t);
  }, [leaving]);

  // A key anywhere, not only on the focused element: nobody tabs to a splash.
  useEffect(() => {
    if (gone) return;
    window.addEventListener("keydown", skip);
    return () => window.removeEventListener("keydown", skip);
  }, [gone, skip]);

  if (gone) return null;

  return (
    <div
      className={`splash${leaving ? " splash-out" : ""}`}
      role="status"
      aria-live="polite"
      onClick={skip}
    >
      <img src={splash} alt="WTFault Scanner" className="splash-art" />
      <div className="splash-status">
        {stalled ? (
          <span className="splash-slow">
            The diagnostic core is taking longer than usual to start…
          </span>
        ) : ready ? (
          // Ready and merely being looked at. Say so rather than claiming to
          // still be starting, which would be a spinner telling a small lie.
          <span className="splash-skip">Ready — click anywhere to continue</span>
        ) : (
          <span className="row" style={{ gap: 10, justifyContent: "center" }}>
            <span className="spin" />
            Starting the diagnostic core
          </span>
        )}
      </div>
    </div>
  );
}
