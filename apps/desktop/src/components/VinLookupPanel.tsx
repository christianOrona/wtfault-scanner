// Looking the connected vehicle's VIN up with NHTSA vPIC, on request only (#55).
//
// The VIN decoder built into the app knows about a dozen manufacturers and never
// works out the model. NHTSA's free vPIC service decodes nearly any VIN sold in
// the US. Asking it sends the VIN to a US government service, so this is a
// button the person presses, never something the app does by itself. A reply is
// kept on the computer, so each VIN is sent at most once unless they refresh.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { VpicHeld } from "../api/types";
import { ErrorBanner, Spinner } from "./primitives";

export function VinLookupPanel({ connected, vin }: { connected: boolean; vin: string }) {
  const [held, setHeld] = useState<VpicHeld | null>(null);
  const [fromCache, setFromCache] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{code: string; message: string} | null>(null);

  const load = useCallback(async () => {
    try {
      const r = await api.vpicStatus();
      setHeld(r.cached);
      setError(null);
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  const lookup = useCallback(async (refresh: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const r = await api.lookupVpic(refresh);
      setHeld(r);
      setFromCache(r.from_cache);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (connected) {
      void load();
    }
  }, [connected, vin, load]);

  if (!connected) return null;

  return (
    <div style={{ marginTop: 8 }}>
      {error && <ErrorBanner error={error} />}
      {held?.decode ? (
        <div>
          <div>{[held.decode.model_year, held.decode.make, held.decode.model].filter(Boolean).join(" ")}</div>
          <div className="faint">{[held.decode.engine, held.decode.fuel].filter(Boolean).join(" · ")}</div>
          <div className="faint" style={{ fontSize: 11 }}>
            From NHTSA vPIC, looked up {new Date(held.fetched_at).toLocaleDateString()}
            {fromCache === true && " (kept on this computer, nothing sent)"}
          </div>
          {held.decode.warning && <div className="faint">NHTSA noted: {held.decode.warning}</div>}
          <button onClick={() => void lookup(true)} disabled={busy}>Refresh</button>
        </div>
      ) : (
        <div>
          <div className="faint">
            The built-in decoder does not know the model. NHTSA's free vPIC service can look this VIN up.
            This sends the VIN, and nothing else, to a US government service.
          </div>
          <button className="primary" onClick={() => void lookup(false)} disabled={busy}>
            {busy ? <Spinner /> : "Look up with NHTSA"}
          </button>
        </div>
      )}
    </div>
  );
}
