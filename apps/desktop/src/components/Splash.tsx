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

import { useEffect, useState } from "react";
import splash from "../assets/splash.png";

export function Splash({ ready, stalled }: { ready: boolean; stalled: boolean }) {
  const [gone, setGone] = useState(false);

  // Fade out rather than cut, and unmount after the transition so the image is
  // not sitting in the tree forever.
  useEffect(() => {
    if (!ready) return;
    const t = window.setTimeout(() => setGone(true), 420);
    return () => window.clearTimeout(t);
  }, [ready]);

  if (gone) return null;

  return (
    <div className={`splash${ready ? " splash-out" : ""}`} role="status" aria-live="polite">
      <img src={splash} alt="WTFault Scanner" className="splash-art" />
      <div className="splash-status">
        {stalled ? (
          <span className="splash-slow">
            The diagnostic core is taking longer than usual to start…
          </span>
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
