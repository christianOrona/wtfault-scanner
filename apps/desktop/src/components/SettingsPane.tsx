// Model providers: add one, test it, choose which the agent uses.
//
// The key field is the sensitive part of this whole application, so two things
// are non-negotiable here: the stored key is never shown (the server only ever
// sends a hint like `sk-…mnop`), and how it is stored is stated on screen
// rather than buried in a README.

import { useCallback, useEffect, useState } from "react";
import { api, describeError, type ProviderInput } from "../api/client";
import type { ProbeResult, ProfilesResponse, ProviderKindId, ProviderView, ProvidersResponse } from "../api/types";
import { ErrorBanner, Spinner } from "./primitives";

const BLANK: ProviderInput = {
  kind: "ollama",
  label: "",
  base_url: "",
  model: "",
  api_key: "",
  speed: "quality",
  max_steps: null,
  context_tokens: null,
};

export function SettingsPane({ onChanged }: { onChanged?: () => void }) {
  const [data, setData] = useState<ProvidersResponse | null>(null);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState<ProviderInput>(BLANK);
  const [probes, setProbes] = useState<Record<string, ProbeResult | "running">>({});

  const load = useCallback(async () => {
    try {
      setData(await api.providers());
      setError(null);
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const kinds = data?.kinds ?? [];
  const kindInfo = kinds.find((k) => k.id === draft.kind);
  // The server decides which endpoints can honour a speed setting, so the
  // control is never offered where it would quietly do nothing.
  const speedSupported = draft.kind === "ollama" || draft.kind === "anthropic";

  function startAdd(kind: ProviderKindId) {
    const info = kinds.find((k) => k.id === kind);
    setEditing("new");
    setDraft({
      kind,
      label: info?.label ?? "",
      base_url: info?.default_base_url ?? "",
      model: kind === "anthropic" ? "claude-opus-5" : "",
      api_key: "",
      speed: "quality",
      max_steps: null,
      context_tokens: null,
    });
  }

  function startEdit(p: ProviderView) {
    setEditing(p.id);
    // No api_key: undefined means "leave the stored one alone". Prefilling a
    // placeholder would risk writing the placeholder back as the key.
    setDraft({
      kind: p.kind,
      label: p.label,
      base_url: p.base_url ?? "",
      model: p.model,
      speed: p.speed,
      max_steps: p.max_steps,
      context_tokens: p.context_tokens,
    });
  }

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const body: ProviderInput = {
        ...draft,
        base_url: draft.base_url?.trim() ? draft.base_url.trim() : null,
      };
      if (editing === "new") await api.addProvider({ ...body, select: true });
      else if (editing) await api.updateProvider(editing, body);
      setEditing(null);
      setDraft(BLANK);
      await load();
      onChanged?.();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  async function act(fn: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
      await load();
      onChanged?.();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  async function test(id: string) {
    setProbes((p) => ({ ...p, [id]: "running" }));
    try {
      const result = await api.testProvider(id);
      setProbes((p) => ({ ...p, [id]: result }));
    } catch (e) {
      const d = describeError(e);
      setProbes((p) => ({ ...p, [id]: { reachable: false, error: d } }));
    }
  }

  const canSave =
    !busy &&
    draft.label.trim() !== "" &&
    draft.model.trim() !== "" &&
    (draft.kind !== "openai_compatible" || !!draft.base_url?.trim()) &&
    // A kind that requires a key needs one, unless one is already stored.
    (!kinds.find((k) => k.id === draft.kind)?.requires_key ||
      !!draft.api_key?.trim() ||
      (editing !== "new" && !!data?.providers.find((p) => p.id === editing)?.has_key));

  return (
    <div className="pane">
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <strong>Model providers</strong>
        {!editing && (
          <div className="row">
            {kinds.map((k) => (
              <button key={k.id} onClick={() => startAdd(k.id)} disabled={busy}>
                Add {k.label}
              </button>
            ))}
          </div>
        )}
      </div>

      <ErrorBanner error={error} />

      {!data?.providers.length && !editing && (
        <div className="card">
          <strong>No model is set up yet.</strong>
          <p className="muted" style={{ marginBottom: 0 }}>
            Without one, the app reads the vehicle but cannot explain it. Add
            <strong> Ollama</strong> to use a machine on your own network, or
            <strong> Anthropic</strong> for the strongest reasoning.
          </p>
        </div>
      )}

      {data?.providers.map((p) => {
        const probe = probes[p.id];
        return (
          <div className="card" key={p.id}>
            <div className="row" style={{ justifyContent: "space-between" }}>
              <div className="row">
                <strong>{p.label}</strong>
                {p.selected && <span className="pill"><span className="dot ready" />in use</span>}
                <span className="tag">{p.kind}</span>
              </div>
              <div className="row">
                <button onClick={() => void test(p.id)} disabled={busy || probe === "running"}>
                  {probe === "running" ? <Spinner label="Testing" /> : "Test"}
                </button>
                {!p.selected && (
                  <button onClick={() => void act(() => api.selectProvider(p.id))} disabled={busy}>
                    Use this
                  </button>
                )}
                <button onClick={() => startEdit(p)} disabled={busy}>Edit</button>
                <button
                  className="danger"
                  onClick={() => void act(() => api.deleteProvider(p.id))}
                  disabled={busy}
                >
                  Remove
                </button>
              </div>
            </div>

            <div className="provenance" style={{ marginTop: 8 }}>
              <span>model {p.model}</span>
              {p.base_url && <span>{p.base_url}</span>}
              <span>{p.has_key ? `key ${p.key_hint}` : "no key"}</span>
              {p.speed_supported && <span>thinking {p.speed === "fast" ? "fast" : "full"}</span>}
              <span>{p.max_steps ?? 14} steps</span>
              {p.context_supported && <span>{(p.context_tokens ?? 16384).toLocaleString()} ctx</span>}
            </div>

            {probe && probe !== "running" && (
              <div
                className={`banner ${probe.reachable ? "info" : "serious"}`}
                style={{ marginTop: 8, marginBottom: 0 }}
              >
                <span className="b-code">{probe.reachable ? "reachable" : "unreachable"}</span>
                <div>
                  {probe.reachable ? (
                    <>
                      <div>
                        Answered in {probe.elapsed_ms} ms
                        {probe.models?.length ? ` — ${probe.models.length} models available` : ""}
                      </div>
                      {/* The server flags a model the endpoint does not have, which
                          is the most common misconfiguration and otherwise only
                          shows up on the first real question. */}
                      {probe.detail && <div style={{ marginTop: 4 }}>{probe.detail}</div>}
                    </>
                  ) : (
                    <div>{probe.error?.message ?? "no response"}</div>
                  )}
                </div>
              </div>
            )}
          </div>
        );
      })}

      {editing && (
        <div className="card" style={{ borderColor: "var(--accent-dim)" }}>
          <h2 style={{ marginTop: 0, fontSize: 14 }}>
            {editing === "new" ? "New provider" : "Edit provider"}
          </h2>

          <div className="field">
            <label>Type</label>
            <select
              value={draft.kind}
              onChange={(e) => {
                const kind = e.target.value as ProviderKindId;
                const info = kinds.find((k) => k.id === kind);
                setDraft((d) => ({ ...d, kind, base_url: info?.default_base_url ?? "" }));
              }}
            >
              {kinds.map((k) => <option key={k.id} value={k.id}>{k.label}</option>)}
            </select>
            {kindInfo && <span className="faint">{kindInfo.help}</span>}
          </div>

          <div className="field">
            <label>Name</label>
            <input
              type="text"
              value={draft.label}
              placeholder="e.g. Jarvis (GPU box)"
              onChange={(e) => setDraft((d) => ({ ...d, label: e.target.value }))}
            />
          </div>

          {/* Anthropic is the only endpoint with no reason to be overridden. */}
          {draft.kind !== "anthropic" && (
            <div className="field">
              <label>Endpoint{draft.kind === "openai_compatible" ? "" : " (optional)"}</label>
              <input
                type="text"
                value={draft.base_url ?? ""}
                placeholder="http://192.168.1.207:11434"
                onChange={(e) => setDraft((d) => ({ ...d, base_url: e.target.value }))}
              />
              <span className="faint">
                Use the machine&apos;s address, not <code>127.0.0.1</code>, when the model runs on
                another box.
              </span>
            </div>
          )}

          <div className="field">
            <label>Model</label>
            <input
              type="text"
              value={draft.model}
              placeholder={draft.kind === "anthropic" ? "claude-opus-5" : "qwen2.5:7b"}
              onChange={(e) => setDraft((d) => ({ ...d, model: e.target.value }))}
            />
          </div>

          {(kindInfo?.requires_key || draft.kind === "openai_compatible") && (
            <div className="field">
              <label>
                API key
                {kindInfo?.requires_key ? "" : " (leave blank if not needed)"}
              </label>
              <input
                type="password"
                value={draft.api_key ?? ""}
                placeholder={
                  editing !== "new" && data?.providers.find((p) => p.id === editing)?.has_key
                    ? "leave blank to keep the stored key"
                    : "sk-ant-…"
                }
                onChange={(e) => setDraft((d) => ({ ...d, api_key: e.target.value }))}
              />
              {data && (
                <span className="faint">
                  {data.storage_note} Stored at <code>{data.settings_path}</code>.
                </span>
              )}
            </div>
          )}

          {/* The two speed levers. Both matter far more than they look: over
              99% of a local inspection is the model deliberating, not the
              adapter reading, so these are what decide whether a scan takes two
              minutes or ten. */}
          <div className="field">
            <label>Thinking</label>
            <div className="seg">
              <button
                aria-pressed={(draft.speed ?? "quality") === "quality"}
                onClick={() => setDraft((d) => ({ ...d, speed: "quality" }))}
                disabled={!speedSupported}
              >
                Full
              </button>
              <button
                aria-pressed={draft.speed === "fast"}
                onClick={() => setDraft((d) => ({ ...d, speed: "fast" }))}
                disabled={!speedSupported}
              >
                Fast
              </button>
            </div>
            <span className="faint">
              {!speedSupported
                ? "Not available for this endpoint: there is no portable field for it, and guessing a vendor extension would fail differently on every server."
                : draft.kind === "ollama"
                  ? "Fast turns off the model's reasoning phase (Qwen3 and similar). Much quicker, and it will miss things."
                  : "Fast lowers the effort level. It does not disable thinking, which Anthropic warns can make a model describe a tool call instead of making one."}
            </span>
          </div>

          <div className="field">
            <label>Step budget</label>
            <input
              type="number"
              min={2}
              max={40}
              value={draft.max_steps ?? ""}
              placeholder="14 (default)"
              style={{ width: 140 }}
              onChange={(e) =>
                setDraft((d) => ({
                  ...d,
                  max_steps: e.target.value === "" ? null : Number(e.target.value),
                }))
              }
            />
            <span className="faint">
              How many turns the agent may take before it must report. Roughly 40 s per step on a
              local 8B model, well under a second on a hosted one. Fewer steps is faster and
              reads less of the vehicle; the last step is always reserved for writing the report.
            </span>
          </div>

          {draft.kind === "ollama" && (
            <div className="field">
              <label>Context window</label>
              <input
                type="number"
                min={2048}
                max={131072}
                step={2048}
                value={draft.context_tokens ?? ""}
                placeholder="16384 (default)"
                style={{ width: 160 }}
                onChange={(e) =>
                  setDraft((d) => ({
                    ...d,
                    context_tokens: e.target.value === "" ? null : Number(e.target.value),
                  }))
                }
              />
              <span className="faint">
                How much of the conversation the model can see. This is the setting that
                decides whether it works at all — Ollama&apos;s own default of 4096 silently
                throws away most of an inspection, and the model then cannot finish. Bigger is
                safer but costs VRAM: too big and the model spills onto the CPU and every step
                gets slower than the last. Raise it if a scan stops early; lower it if each
                step is slow.
              </span>
            </div>
          )}

          <div className="row" style={{ justifyContent: "flex-end", marginTop: 12 }}>
            <button onClick={() => { setEditing(null); setDraft(BLANK); }} disabled={busy}>
              Cancel
            </button>
            <button className="primary" onClick={() => void save()} disabled={!canSave}>
              {busy ? <Spinner label="Saving" /> : "Save"}
            </button>
          </div>
        </div>
      )}

      <VehicleProfiles />
    </div>
  );
}

/**
 * What user-supplied vehicle profiles contributed at startup.
 *
 * Shown because silent loading is indistinguishable from not loading. Someone
 * who drops a file in that folder and restarts needs to see either "read it,
 * here is what it added" or "could not read it, here is why" — anything else
 * and they are debugging by guesswork.
 */
function VehicleProfiles() {
  const [data, setData] = useState<ProfilesResponse | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      setData(await api.profiles());
    } catch {
      /* Profiles are optional; a failure to read the report is not fatal. */
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const files = data?.report.files ?? [];
  const failures = files.filter((f) => f.error);

  return (
    <div className="section" style={{ marginTop: 26 }}>
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 8 }}>
        <h2 style={{ margin: 0 }}>Vehicle profiles</h2>
        <button className="mini" onClick={() => void load()} disabled={busy}>
          {busy ? <Spinner /> : "Re-read"}
        </button>
      </div>

      <div className="explain">
        Extra vehicle knowledge you can give the app without reinstalling it. Drop a YAML
        file in the folder below and restart — it can add readings this build cannot decode,
        plain-language explanations, or the location of a configuration setting you have
        verified on your own vehicle. Everything loaded this way is labelled on screen with
        the file it came from. The folder contains a README with the format and the method
        for working a mapping out safely.
      </div>

      {data?.report.directory && (
        <div className="card" style={{ marginTop: 10 }}>
          <div className="faint" style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}>
            Folder
          </div>
          <div className="mono" style={{ fontSize: 12, wordBreak: "break-all" }}>
            {data.report.directory}
          </div>
        </div>
      )}

      {failures.length > 0 && (
        <div className="banner serious" style={{ marginTop: 10 }}>
          <span className="b-code">not loaded</span>
          <div>
            {failures.length} file{failures.length === 1 ? "" : "s"} could not be read. The app
            started anyway; these simply contributed nothing.
          </div>
        </div>
      )}

      {files.length > 0 ? (
        <table style={{ marginTop: 10 }}>
          <thead>
            <tr><th>File</th><th>Kind</th><th>Added</th></tr>
          </thead>
          <tbody>
            {files.map((f) => (
              <tr key={f.path}>
                <td className="mono" style={{ fontSize: 12 }}>{f.source.replace(/^user:/, "")}</td>
                <td>{f.kind}</td>
                <td>
                  {f.error ? (
                    <span style={{ color: "var(--serious)" }}>{f.error}</span>
                  ) : (
                    `${f.loaded} definition${f.loaded === 1 ? "" : "s"}`
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <div className="faint" style={{ marginTop: 10, fontSize: 12 }}>
          No profile files yet. The app is running on what it shipped with:{" "}
          {data?.feature_count ?? 0} catalogued vehicle settings.
        </div>
      )}
    </div>
  );
}
