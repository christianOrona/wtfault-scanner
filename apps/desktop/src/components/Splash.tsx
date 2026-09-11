// The first eight seconds.
//
// Not decoration. The desktop shell starts the HTTP core and the window at the
// same time, and the window usually wins the race — so the first thing anyone
// saw was an empty grey frame with no name on it while the core came up. That
// reads as a broken app for exactly as long as it takes to start.
//
// # Shape
//
// A card, not a full-screen takeover: the artwork at a readable size with a
// strip of small print beneath it, and the rest of the desktop still visible
// behind. A splash that fills the screen implies the app has taken the machine
// over, which for a few seconds of startup it has not.
//
// The artwork carries the name and the tagline already, so nothing is typed
// over the top of it. A splash that states the product name twice in two
// different typefaces looks like two designs arguing.
//
// # It closes itself
//
// Nobody should have to click a splash screen. It leaves on its own once two
// things are true: the core has answered, and it has been up long enough to
// read. There is no skip, deliberately — the links in it are clickable, and an
// overlay that both dismisses on click and contains links is an overlay that
// swallows the link the person was aiming at.
//
// # The progress is real
//
// It reports the boot steps that are actually happening, named, as they
// finish. A progress bar that animates to 100% on a timer is a lie told
// smoothly, and it is exactly as useless as no progress bar at all when
// something is genuinely stuck.

import { useEffect, useState } from "react";
import splash from "../assets/splash.jpg";
import { LICENCE, LINKS, PRODUCT_NAME } from "../branding";

/** How long the artwork stays up even when the core is ready sooner.
 *
 * A floor, not a delay: it never holds a *slow* start open any longer than it
 * already would be. A constant rather than a number buried in a component,
 * because it is the kind of value somebody will want to change without going
 * looking for it. */
const MIN_VISIBLE_MS = 8000;

/** One thing that has to happen before the application is usable. */
export interface BootStep {
  /** Stable id. */
  id: string;
  /** What is happening, in the present tense. */
  label: string;
  /** Whether it has finished — succeeded or failed, but no longer pending. */
  done: boolean;
}

export function Splash({
  ready,
  stalled,
  steps,
  version,
}: {
  /** Whether the diagnostic core has answered. */
  ready: boolean;
  /** Whether it has been failing long enough to be worth mentioning. */
  stalled: boolean;
  /** The boot work, in the order it happens. */
  steps: BootStep[];
  /** Build version, once the core has reported one. */
  version?: string | null;
}) {
  const [gone, setGone] = useState(false);
  const [minElapsed, setMinElapsed] = useState(false);

  useEffect(() => {
    const t = window.setTimeout(() => setMinElapsed(true), MIN_VISIBLE_MS);
    return () => window.clearTimeout(t);
  }, []);

  // Leaving needs both: the core has answered, and the card has been up long
  // enough to see. The floor never reveals an application that is not running
  // yet — it only ever delays one that is.
  const leaving = ready && minElapsed;

  useEffect(() => {
    if (!leaving) return;
    // Fade out rather than cut, and unmount after the transition so a
    // megabyte of artwork is not sitting in the tree for the session.
    const t = window.setTimeout(() => setGone(true), 420);
    return () => window.clearTimeout(t);
  }, [leaving]);

  if (gone) return null;

  const finished = steps.filter((s) => s.done).length;
  const current = steps.find((s) => !s.done);
  // Against the steps actually declared, so the bar cannot reach the end while
  // something is still running.
  const percent = steps.length ? Math.round((finished / steps.length) * 100) : 0;

  return (
    <div className={`splash-scrim${leaving ? " splash-out" : ""}`} role="status" aria-live="polite">
      <div className="splash-card">
        {/* The artwork is the card. It already carries the name and the
            tagline, so nothing here repeats them — a splash that says the
            product name twice in two different typefaces looks like two
            designs arguing. The card takes its shape from the image, so
            swapping a square for a banner needs no CSS. */}
        <img src={splash} alt={PRODUCT_NAME} className="splash-art" />

        <div className="splash-strip">
          <div className="splash-legend">
            <div className="splash-meta">
              {version ? `Version ${version}` : PRODUCT_NAME} · {LICENCE} · ©&nbsp;2026 Christian
              Orona
            </div>
            {/* Opened in the person's own browser. Nothing here is fetched by
                the app, and the full URL sits on the title so hovering says
                where a link goes before it is followed. */}
            <div className="splash-links">
              {LINKS.map((l) => (
                <a
                  key={l.href}
                  href={l.href}
                  target="_blank"
                  rel="noopener noreferrer"
                  title={l.href}
                >
                  {l.label}
                </a>
              ))}
            </div>
          </div>

          {/* The corner: what is happening, and how far through it is. The ring
              turns the whole time — steps can sit for seconds, and a screen
              with nothing moving on it looks like a screen that has hung. */}
          <div className="splash-progress">
            <div className="splash-working">
              <span
                className={`splash-ring${stalled ? " stalled" : !current ? " done" : ""}`}
                aria-hidden="true"
              />
              <span className={`splash-step${stalled ? " splash-slow" : ""}`}>
                {stalled ? "Core is slow to start…" : current ? current.label : "Ready"}
              </span>
            </div>
            <div className="splash-count">
              {finished} of {steps.length}
            </div>
          </div>
        </div>

        <div className="splash-bar" aria-hidden="true">
          <div
            className={`splash-bar-fill${stalled ? " stalled" : ""}`}
            style={{ width: `${percent}%` }}
          />
        </div>
      </div>
    </div>
  );
}
