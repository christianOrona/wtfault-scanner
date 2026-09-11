// Turning release notes into something worth reading in a banner.
//
// The notes are written as markdown for GitHub, where somebody has chosen to
// read them. In the app they arrive unasked, on top of whatever the person
// opened the app to do, so they get a different treatment: the claim of each
// entry, and the supporting paragraph underneath it in smaller type.
//
// Rendering the raw text in a <pre> — which is what this replaced — showed the
// asterisks, the hard-wrapped line breaks at column 78, and the headings, in a
// monospace block that looked like log output. Notes written for one medium do
// not survive being pasted into another.
//
// This is not a markdown renderer and should not grow into one. It reads the
// shape our own notes are written in. Anything it does not recognise falls
// through as plain text rather than being dropped, because a release note that
// silently vanishes is worse than one that looks plain.

export type Highlight = {
  /** The claim, in a sentence. Bold in the source, or the first sentence. */
  headline: string;
  /** What is behind the claim. May be empty. */
  body: string;
};

/** Markdown emphasis, once the structure has been read off it. */
function plain(text: string): string {
  return text
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/`(.+?)`/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/** Split one entry into its claim and its explanation. */
function split(text: string): Highlight {
  const bold = text.match(/^\s*\*\*(.+?)\*\*\s*/);
  if (bold) {
    return { headline: plain(bold[1]), body: plain(text.slice(bold[0].length)) };
  }
  // No bold lead: the first sentence is the claim. The lookbehind would be
  // neater but is not safe on every engine this ships to.
  const end = text.search(/[.!?](\s|$)/);
  if (end > 0 && end < text.length - 1) {
    return { headline: plain(text.slice(0, end + 1)), body: plain(text.slice(end + 1)) };
  }
  return { headline: plain(text), body: "" };
}

/**
 * The notes, as a handful of entries.
 *
 * Headings ("### Fixed") are dropped: the banner has already said a new version
 * exists, and a section label with one item under it is a table of contents for
 * a sentence.
 */
export function highlightsOf(notes: string, limit = 4): Highlight[] {
  const blocks: string[] = [];
  let current = "";

  for (const raw of notes.split(/\r?\n/)) {
    const line = raw.trimEnd();
    const isHeading = /^#{1,6}\s/.test(line);
    const isBullet = /^\s*[-*]\s+/.test(line);

    if (line.trim() === "" || isHeading || isBullet) {
      if (current.trim()) blocks.push(current);
      current = isBullet ? line.replace(/^\s*[-*]\s+/, "") : "";
      if (isHeading) current = "";
      continue;
    }
    // A continuation line. The notes are hard-wrapped, so joining with a space
    // is what turns column-78 prose back into sentences.
    current = current ? `${current} ${line.trim()}` : line.trim();
  }
  if (current.trim()) blocks.push(current);

  return blocks
    .map(split)
    .filter((h) => h.headline.length > 0)
    .slice(0, limit);
}
