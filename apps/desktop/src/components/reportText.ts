// Render a report as plain text you can paste into a message to a mechanic.
//
// Handoff section 1 lists "give me a mechanic-style report I can send to a shop"
// as a core user story, and until now there was no way to get a report out of
// the window at all.
//
// Plain text rather than PDF or HTML on purpose: it pastes into a text message,
// an email, a WhatsApp to a seller, or a garage's booking form without anything
// being lost. The formatting has to survive being pasted somewhere that strips
// all of it, so structure is carried by line breaks and dashes, never by markup.
//
// The source labels survive too. A report that reaches a third party without
// them would present a cost guess as a measurement, which is exactly the thing
// this project refuses to do on screen.

import type { InspectResponse, Report, Finding } from "../api/types";

const SOURCE_NOTE: Record<string, string> = {
  measured: "measured from this vehicle",
  catalog: "standard description for this code",
  model_knowledge: "AI general knowledge, NOT verified against this vehicle",
};

const VERDICT_LINE: Record<Report["verdict"], string> = {
  walk_away: "WALK AWAY - serious faults found",
  negotiate: "NEGOTIATE - real problems with a knowable cost",
  looks_sound: "LOOKS SOUND - nothing alarming in what could be checked",
  inconclusive: "INCONCLUSIVE - too little could be read to say",
};

function finding(f: Finding, i: number): string {
  const lines = [
    `${i}. ${f.title}  [${f.severity}]`,
    `   ${f.plain_english}`,
    `   Source: ${SOURCE_NOTE[f.source] ?? f.source}`,
  ];
  if (f.evidence?.length) lines.push(`   Evidence: ${f.evidence.join("; ")}`);
  if (f.what_to_do) lines.push(`   What to do: ${f.what_to_do}`);
  if (f.estimated_cost) {
    lines.push(
      `   Rough cost: ${f.estimated_cost.currency} ${Math.round(f.estimated_cost.low)}-${Math.round(
        f.estimated_cost.high,
      )} (estimate only - ${f.estimated_cost.basis})`,
    );
  }
  return lines.join("\n");
}

/** Everything a shop would want, and the caveats they need with it. */
export function reportToText(
  result: InspectResponse,
  vehicle: { vin?: string | null; descriptor?: string | null } = {},
): string {
  const r = result.report;
  if (!r) return "No report was produced.";

  const out: string[] = [];
  out.push("AI MECHANIC - PRE-PURCHASE INSPECTION");
  out.push(new Date().toLocaleString());
  if (vehicle.vin) out.push(`VIN: ${vehicle.vin}`);
  if (vehicle.descriptor) out.push(`Adapter: ${vehicle.descriptor}`);
  out.push("");

  // Same distinction the screen makes: "inconclusive" with findings means real
  // faults were found and the rest could not be judged, which is not the same
  // as having read nothing. Printing "too little could be read" above a list of
  // serious faults would misrepresent the report to whoever receives it.
  out.push(
    r.verdict === "inconclusive" && r.findings?.length
      ? "INCONCLUSIVE - faults found, but not enough of the vehicle could be read to judge it overall"
      : (VERDICT_LINE[r.verdict] ?? r.verdict),
  );
  out.push(r.headline);
  out.push("");
  out.push(r.summary);

  if (result.truncated) {
    out.push("");
    out.push(
      "NOTE: the inspection did not finish. What follows is the evidence gathered\n" +
        "before it stopped, not a completed assessment.",
    );
  }

  if (r.findings?.length) {
    out.push("", "WHAT WAS FOUND", "--------------");
    r.findings.forEach((f, i) => out.push(finding(f, i + 1), ""));
  }

  if (r.watch_items?.length) {
    out.push("LIKELY TO NEED ATTENTION LATER", "------------------------------");
    out.push("Predictions, not observations. Nothing below was seen failing.");
    out.push("");
    r.watch_items.forEach((f, i) => out.push(finding(f, i + 1), ""));
  }

  if (r.not_checked?.length) {
    out.push("WHAT THIS SCAN COULD NOT CHECK", "------------------------------");
    r.not_checked.forEach((n) => out.push(`- ${n}`));
    out.push("");
  }

  if (r.next_steps?.length) {
    out.push("SUGGESTED NEXT STEPS", "--------------------");
    r.next_steps.forEach((n, i) => out.push(`${i + 1}. ${n}`));
    out.push("");
  }

  // The limits go at the bottom, where a reader who skimmed still meets them.
  out.push("ABOUT THIS REPORT", "-----------------");
  out.push(
    "Produced by plugging into the vehicle's diagnostic port and reading its own",
    "electronics. It CANNOT see brakes, tyres, suspension, rust, gearbox wear, or",
    "anything mechanical without a sensor. A clean report here is not a clean bill",
    "of health, and is not a substitute for a physical inspection.",
    "",
    "Anything marked \"AI general knowledge\" was not measured from this vehicle.",
    "Cost figures are rough ranges, never quotes.",
    "",
    `${result.steps} diagnostic steps. Read-only: nothing was changed on the vehicle.`,
  );

  return out.join("\n");
}
