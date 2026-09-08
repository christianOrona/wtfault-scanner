// Getting data out of the window.
//
// The obvious implementation is a blob URL and an `<a download>` click. Inside
// the desktop shell that does nothing at all: a Tauri webview has no download
// manager, so the click is swallowed with no file and no error. The button
// looked like it worked, which is worse than a button that visibly fails.
//
// So the core writes the file instead. It runs in the same process, it already
// serves this page, and it can report the actual path — which means the app can
// say "saved to C:\Users\you\Downloads\..." rather than leaving someone hunting
// through folders for a file that may never have existed.
//
// Two formats, chosen for what people do with them:
//
//   .txt   the report, as prose. Pastes into a message to a mechanic.
//   .csv   readings and tables, for a spreadsheet.
//
// Deliberately NOT PDF. Generating one in the browser means shipping a large
// library to produce a file that is harder to search, harder to diff, and
// harder to paste from than the text it was made out of. Print-to-PDF is one
// keystroke away and produces a better result than anything this app would.

import { api } from "../api/client";

/** Where a saved file ended up, for telling the user. */
export interface Saved {
  path: string;
  filename: string;
}

/** Ask the core to write a file, and return where it put it. */
export async function saveFile(
  filename: string,
  content: string,
): Promise<Saved> {
  return api.exportFile({ filename, content });
}

/**
 * A filename that sorts chronologically and says what it is.
 *
 * The VIN is included when known: a folder of these a year from now is useless
 * if they are all called `report.txt`, and the VIN is the one thing that says
 * which vehicle a scan came from.
 */
export function scanFilename(kind: string, vin: string | null, ext: string): string {
  const now = new Date();
  const stamp = [
    now.getFullYear(),
    String(now.getMonth() + 1).padStart(2, "0"),
    String(now.getDate()).padStart(2, "0"),
    "-",
    String(now.getHours()).padStart(2, "0"),
    String(now.getMinutes()).padStart(2, "0"),
  ].join("");
  const id = vin ? `-${vin}` : "";
  return `wtfault-${kind}${id}-${stamp}.${ext}`;
}

/** One CSV field, quoted only when it has to be. */
function field(v: unknown): string {
  const s = v == null ? "" : String(v);
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/**
 * Rows to CSV.
 *
 * A leading UTF-8 byte-order mark, because Excel on Windows reads a BOM-less
 * UTF-8 file as the system codepage and turns every degree sign into mojibake.
 * The file is correct either way; only Excel needs telling.
 */
export function toCsv(headers: string[], rows: unknown[][]): string {
  const lines = [headers.map(field).join(",")];
  for (const r of rows) lines.push(r.map(field).join(","));
  return `\uFEFF${lines.join("\r\n")}\r\n`;
}
