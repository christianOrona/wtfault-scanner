// The page behind /hologram-preview.html. Development only: see that file.
//
// It shows the same component the app shows, so a screenshot or a recording
// taken here is of the real thing and not of something drawn for the README.

import { createRoot } from "react-dom/client";
import type { Region } from "../api/types";
import VehicleHologram from "../components/VehicleHologram";
import "../styles.css";
import type { BodyShape } from "./vehicles";

const query = new URLSearchParams(location.search);
const region = (query.get("region") ?? "exhaust") as Region;
const part = query.get("part");
const what = query.get("what") ?? part ?? region;
const only = query.get("shape") as BodyShape | null;
const record = query.has("record");

// On this page `?record` is enough to ask for a recording. In the app only the
// recording script can, by setting the list before the page loads.
if (record) window.holoRecording = [];

function Preview() {
  const shapes: BodyShape[] = only ? [only] : ["car", "suv", "truck", "van"];
  return (
    <div
      className={record ? "holo-record" : undefined}
      style={
        record
          ? undefined
          : { display: "grid", gridTemplateColumns: only ? "1fr" : "1fr 1fr", gap: 12, padding: 12 }
      }
    >
      {shapes.map((shape) => (
        <VehicleHologram
          key={shape}
          region={region}
          partId={part}
          what={what}
          startShape={shape}
        />
      ))}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Preview />);
