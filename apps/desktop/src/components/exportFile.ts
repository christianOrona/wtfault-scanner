// Getting data out of the window.
//
// Copy-to-clipboard was the only way out, and it only worked for the inspection
// report. Everything else the app reads - the live readings, the codes, the
// self-test results, the raw adapter transcript - could be looked at and not
// kept. For anything you want to send to a shop, keep for next year, or open in
// a spreadsheet, that is the same as not having it.
//
// Two formats, chosen for what people actually do with them:
//
//   .txt   the report, as prose. Pastes into a message to a mechanic without
//          markup, which is exactly what the report is for.
//   .csv   readings and tables, for a spreadsheet.
//
// Deliberately NOT PDF. Generating one in the browser means shipping a large
// library to produce a file that is harder to search, harder to diff, and
// harder to paste from than the text it was made out of. Printing to PDF is one
// keystroke away in every browser and produces a better result than anything
// this app would build. If the report needs to look like a document, print it.

/** Trigger a download of `content` as `filename`. */
export function download(filename: string, content: string, mime: string): void {
  const blob = new Blob([content], { type: `${mime};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  // Revoking immediately can cancel the download in some browsers; a tick is
  // enough for the navigation to have started.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * A filename that sorts chronologically and says what it is.
 *
 * The VIN is included when known: a folder of these a year from now is useless
 * if they are all called `report.txt`, and the one thing that identifies which
 * vehicle a scan came from is the VIN.
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
  return `wrenchgpt-${kind}${id}-${stamp}.${ext}`;
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
 * UTF-8 file as the system codepage and turns every degree sign and every
 * accented character into mojibake. The file is correct either way; only Excel
 * needs telling.
 */
export function toCsv(headers: string[], rows: unknown[][]): string {
  const lines = [headers.map(field).join(",")];
  for (const r of rows) lines.push(r.map(field).join(","));
  return `﻿${lines.join("\r\n")}\r\n`;
}
