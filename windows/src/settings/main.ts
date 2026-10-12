// Settings window — the place where anything that writes to disk is confirmed.
// Stage 2 covers the Claude Code hooks and the general preferences; API keys and
// integrations land here too in a later stage.

import "./settings.css";
import { Bridge, onEvent, type HookStatus } from "../core/bridge";
import { DEFAULT_SETTINGS, type Settings } from "../core/state";
import { h, clear } from "../views/dom";

let settings: Settings = { ...DEFAULT_SETTINGS };
let version = "";

// Track loading state for model fetching
let loadingModels: Record<string, boolean> = {
  anthropic: false,
  google: false,
  openai: false,
  custom: false,
};

const root = document.getElementById("settings-root")!;

async function save() {
  await Bridge.saveSettings(settings);
}

// ── Reusable bits ─────────────────────────────────────────────────────────────

function toggle(on: boolean, onChange: (v: boolean) => void): HTMLElement {
  const el = h("button", { class: on ? "switch on" : "switch", "aria-pressed": on });
  el.addEventListener("click", () => {
    const next = !el.classList.contains("on");
    el.classList.toggle("on", next);
    onChange(next);
  });
  return el;
}

function statusDot(ok: boolean): HTMLElement {
  return h("i", { class: "dot", style: `background:${ok ? "#22c55e" : "#f4505e"}` });
}

function renderDiff(text: string): HTMLElement {
  const box = h("div", { class: "diff" });
  for (const line of text.split("\n")) {
    const cls = line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "ctx";
    box.append(h("div", { class: cls, text: line }));
  }
  return box;
}

// ── Claude Code section ───────────────────────────────────────────────────────

function claudeSection(status: HookStatus): HTMLElement {
  const body = h("div", { style: "display:flex;flex-direction:column;gap:12px" });
  const section = h(
    "section",
    {},
    h("h2", {}, statusDot(status.installed), h("span", { text: "Claude Code" })),
    body,
  );

  const rebuild = async () => {
    const fresh = await Bridge.hooksStatus();
    if (fresh) Object.assign(status, fresh);
    clear(body);
    draw();
    const head = section.querySelector("h2")!;
    clear(head);
    head.append(statusDot(status.installed), h("span", { text: "Claude Code" }));
  };

  function draw() {
    body.append(
      h("div", {
        class: "hint",
        text: status.installed
          ? "Coucou is hooked into your Claude Code sessions. Tool calls, questions and permission requests show up in the island, and you can answer them there."
          : "Install the hooks to see your Claude Code sessions in the island and approve permissions without leaving what you are doing.",
      }),
      h("div", { class: "row" },
        h("label", { text: "settings.json" }),
        h("span", { class: "path", text: status.settingsPath }),
      ),
      h("div", { class: "row" },
        h("label", { text: "Relay" }),
        h("span", { class: "path", text: status.hookPath }),
        statusDot(status.hookReady),
      ),
    );

    if (!status.hookReady) {
      body.append(h("div", {
        class: "notice warn",
        text: "coucou-hook.exe is not in place yet. Restart Coucou; if it still fails, build it with `cargo build -p coucou-hook`.",
      }));
    }

    const actions = h("div", { class: "row" });
    const install = h("button", {
      class: "primary",
      text: status.installed ? "Reinstall hooks…" : "Install hooks…",
      onclick: () => showPreview(true),
    });
    // Writing hook commands that point at a relay which isn't there would give
    // every Claude Code session a broken hook and nothing to show for it.
    if (!status.hookReady) {
      install.disabled = true;
      install.title = "The relay isn't installed yet.";
    }
    actions.append(install);
    if (status.installed) {
      actions.append(h("button", {
        class: "danger",
        text: "Uninstall hooks…",
        onclick: () => showPreview(false),
      }));
    }
    body.append(actions);
  }

  async function showPreview(install: boolean) {
    let preview;
    try {
      preview = await Bridge.hooksPreview(install);
    } catch (err) {
      // An unreadable or invalid settings.json stops here rather than being
      // treated as empty and written over.
      clear(body);
      body.append(
        h("div", { class: "notice err", text: String(err).replace(/^Error:\s*/, "") }),
        h("div", { class: "row" }, h("button", {
          text: "Back",
          onclick: () => { clear(body); draw(); },
        })),
      );
      return;
    }
    if (!preview) return;
    clear(body);
    body.append(
      h("div", {
        class: "hint",
        text: install
          ? "This is exactly what will change in your settings.json. Your own hooks are left untouched."
          : "This removes Coucou's entries only. Your own hooks are left untouched.",
      }),
      renderDiff(preview.diff),
      h("div", { class: "row" },
        h("span", { class: "path", text: `Backup → ${preview.backup}` }),
      ),
    );
    const confirm = h("button", {
      class: install ? "primary" : "danger",
      text: install ? "Back up and write" : "Back up and remove",
    });
    confirm.addEventListener("click", async () => {
      confirm.disabled = true;
      try {
        const backup = await Bridge.hooksApply(install, preview.fingerprint);
        clear(body);
        body.append(h("div", {
          class: "notice ok",
          text: `Done. Previous settings saved as ${backup}. Open a new Claude Code session to pick the hooks up.`,
        }));
        window.setTimeout(() => void rebuild(), 2600);
      } catch (err) {
        confirm.disabled = false;
        body.append(h("div", { class: "notice err", text: `Could not write: ${String(err)}` }));
      }
    });
    body.append(h("div", { class: "row" }, confirm, h("button", {
      text: "Cancel",
      onclick: () => { clear(body); draw(); },
    })));
  }

  draw();
  return section;
}

// ── Chat Providers section ──────────────────────────────────────────────────────

const PROVIDER_INFO: Record<string, { name: string; color: string; key: string; baseUrl?: string }> = {
  anthropic: { name: "Anthropic", color: "#E07950", key: "anthropic-api-key" },
  google:    { name: "Google AI", color: "#4285F4", key: "google-api-key" },
  openai:    { name: "OpenAI", color: "#10A37F", key: "openai-api-key" },
  custom:    { name: "Custom (OpenAI-compatible)", color: "#8B5CF6", key: "custom-api-key", baseUrl: "custom-base-url" },
};

async function chatProvidersSection(): Promise<HTMLElement> {
  const section = h("section", {});
  const title = h("h2", {}, h("span", { text: "Chat providers" }));
  const container = h("div", { style: "display:flex;flex-direction:column;gap:16px" });
  
  for (const [providerId, info] of Object.entries(PROVIDER_INFO)) {
    const isCustom = providerId === "custom";
    const providerSection = h("div", { style: "display:flex;flex-direction:column;gap:8px;padding:12px;border:1px solid var(--border);border-radius:8px" });
    
    // Provider header
    const header = h("div", { style: "display:flex;align-items:center;gap:8px" });
    header.append(
      h("i", { class: "dot", style: `background:${info.color}` }),
      h("span", { style: "font-weight:600;font-size:13px", text: info.name }),
      h("span", { class: "hint", style: "margin-left:auto;font-size:11px", text: providerId === settings.chatProvider ? "Active" : "" })
    );
    providerSection.append(header);
    
    // API Key
    const keyRow = h("div", { class: "row", style: "align-items:center" });
    const keyField = h("input", {
      type: "password",
      placeholder: "••••••••••••  (stored)",
      style: "flex:1 1 auto;min-width:0",
      autocomplete: "off",
      spellcheck: "false",
    }) as HTMLInputElement;
    const keySaveBtn = h("button", { class: "primary", text: "Save", style: "width:72px" });
    const keyClearBtn = h("button", { class: "danger", text: "Remove", style: "width:72px;display:none" });
    const keyDot = statusDot(false);
    const keyFeedback = h("div", {});
    
    async function refreshKey() {
      const present = (await Bridge.secretPresent(info.key)) ?? false;
      keyDot.style.background = present ? "#22c55e" : "#f4505e";
      keyField.placeholder = present ? "••••••••••••  (stored)" : (isCustom ? "freellmapi-..." : "sk-...");
      keyClearBtn.style.display = present ? "" : "none";
    }
    
    keySaveBtn.addEventListener("click", async () => {
      const value = keyField.value.trim();
      if (!value) return;
      clear(keyFeedback);
      try {
        await Bridge.secretSet(info.key, value);
        keyField.value = "";
        keyFeedback.append(h("div", { class: "notice ok", text: "Saved." }));
        await refreshKey();
      } catch (err) {
        keyFeedback.append(h("div", { class: "notice err", text: `Could not save: ${String(err)}` }));
      }
    });
    
    keyClearBtn.addEventListener("click", async () => {
      clear(keyFeedback);
      try {
        await Bridge.secretClear(info.key);
        keyFeedback.append(h("div", { class: "notice ok", text: "Key removed." }));
        await refreshKey();
      } catch (err) {
        keyFeedback.append(h("div", { class: "notice err", text: `Could not remove: ${String(err)}` }));
      }
    });
    
    keyRow.append(
      h("label", { style: "min-width:104px", text: "API key" }),
      keyField, keySaveBtn, keyClearBtn, keyDot
    );
    providerSection.append(keyRow);
    providerSection.append(keyFeedback);
    
    // Custom base URL
    if (isCustom) {
      const urlRow = h("div", { class: "row", style: "align-items:center" });
      const urlField = h("input", {
        type: "text",
        value: settings.customBaseUrl,
        placeholder: "http://localhost:3001/v1",
        style: "flex:1 1 auto;min-width:0",
        autocomplete: "off",
        spellcheck: "false",
      }) as HTMLInputElement;
      const urlSaveBtn = h("button", { class: "primary", text: "Save", style: "width:72px" });
      
      urlSaveBtn.addEventListener("click", async () => {
        settings.customBaseUrl = urlField.value.trim();
        await save();
      });
      
      urlRow.append(
        h("label", { style: "min-width:104px", text: "Base URL" }),
        urlField, urlSaveBtn
      );
      providerSection.append(urlRow);
    }
    
    // Model selection
    const modelRow = h("div", { class: "row", style: "align-items:center" });
    const modelSelect = h("select", { style: "flex:1 1 auto;min-width:0" }) as HTMLSelectElement;
    const modelFetchBtn = h("button", { class: "secondary", text: "Fetch models", style: "width:110px" });
    const modelSpinner = h("i", { class: "spinner", style: "width:16px;height:16px;display:none;border:2px solid var(--border);border-top-color:var(--accent);border-radius:50%;animation:spin 1s linear infinite" });
    const modelFeedback = h("div", {});
    
    // Add spinner styles if not present
    if (!document.getElementById("spinner-style")) {
      const style = document.createElement("style");
      style.id = "spinner-style";
      style.textContent = "@keyframes spin { to { transform: rotate(360deg); } }";
      document.head.appendChild(style);
    }
    
    async function refreshModels() {
      if (loadingModels[providerId]) return;
      loadingModels[providerId] = true;
      modelFetchBtn.disabled = true;
      modelFetchBtn.style.display = "none";
      modelSpinner.style.display = "inline-block";
      clear(modelFeedback);
      clear(modelSelect);
      
      try {
        const resp = await Bridge.fetchModels(providerId);
        for (const m of resp.models) {
          modelSelect.append(h("option", { value: m.id, text: m.label }));
        }
        // Select current model
        const currentModel = settings[providerId === "anthropic" ? "model" : 
                                     providerId === "google" ? "googleModel" :
                                     providerId === "openai" ? "openaiModel" : "customModel"];
        if (resp.models.some(m => m.id === currentModel)) {
          modelSelect.value = currentModel;
        } else if (resp.models.length > 0) {
          modelSelect.value = resp.models[0].id;
          // Update settings with first model
          if (providerId === "anthropic") settings.model = resp.models[0].id;
          else if (providerId === "google") settings.googleModel = resp.models[0].id;
          else if (providerId === "openai") settings.openaiModel = resp.models[0].id;
          else settings.customModel = resp.models[0].id;
          await save();
        }
      } catch (err) {
        modelFeedback.append(h("div", { class: "notice err", text: `Failed to fetch: ${String(err)}` }));
      } finally {
        loadingModels[providerId] = false;
        modelFetchBtn.disabled = false;
        modelFetchBtn.style.display = "";
        modelSpinner.style.display = "none";
      }
    }
    
    modelFetchBtn.addEventListener("click", refreshModels);
    
    modelSelect.addEventListener("change", async () => {
      if (providerId === "anthropic") settings.model = modelSelect.value;
      else if (providerId === "google") settings.googleModel = modelSelect.value;
      else if (providerId === "openai") settings.openaiModel = modelSelect.value;
      else settings.customModel = modelSelect.value;
      await save();
    });
    
    modelRow.append(
      h("label", { style: "min-width:104px", text: "Model" }),
      modelSelect, modelFetchBtn, modelSpinner
    );
    providerSection.append(modelRow);
    providerSection.append(modelFeedback);
    
    // Active radio
    const activeRow = h("div", { class: "row", style: "align-items:center" });
    const activeRadio = h("input", { type: "radio", name: "chat-provider", checked: settings.chatProvider === providerId }) as HTMLInputElement;
    activeRadio.addEventListener("change", async () => {
      settings.chatProvider = providerId;
      await save();
      // Re-render to update active indicator
      clear(root);
      await drawAll();
    });
    activeRow.append(
      h("label", { style: "min-width:104px", text: "Use for chat" }),
      activeRadio
    );
    providerSection.append(activeRow);
    
    container.append(providerSection);
    await refreshKey();
  }
  
  section.append(title, container);
  return section;
}

// ── Integrations section ──────────────────────────────────────────────────────

// ... rest of the file

interface IntegrationDef {
  id: string;
  name: string;
  color: string;
  /** Credential Manager keys, in the order they are shown. */
  fields: { key: string; label: string; placeholder: string; secret: boolean }[];
}

const INTEGRATIONS: IntegrationDef[] = [
  { id: "integration_stripe", name: "Stripe", color: "#0570DE",
    fields: [{ key: "stripe-api-key", label: "Secret key", placeholder: "sk_live_…", secret: true }] },
  { id: "integration_github", name: "GitHub", color: "#F4505E",
    fields: [{ key: "github-token", label: "Token", placeholder: "ghp_…", secret: true }] },
  { id: "integration_vercel", name: "Vercel", color: "#7C5CFF",
    fields: [{ key: "vercel-token", label: "Token", placeholder: "…", secret: true }] },
  { id: "integration_n8n", name: "n8n", color: "#F29B38",
    fields: [
      { key: "n8n-url", label: "Instance URL", placeholder: "https://n8n.example.com", secret: false },
      { key: "n8n-api-key", label: "API key", placeholder: "…", secret: true },
    ] },
  { id: "integration_resend", name: "Resend", color: "#22C55E",
    fields: [{ key: "resend-api-key", label: "API key", placeholder: "re_…", secret: true }] },
  { id: "integration_notion", name: "Notion", color: "#8C8C8C",
    fields: [{ key: "notion-api-key", label: "Integration token", placeholder: "ntn_…", secret: true }] },
  { id: "integration_calcom", name: "Cal.com", color: "#C9956A",
    fields: [{ key: "calcom-api-key", label: "API key", placeholder: "cal_…", secret: true }] },
];

const MAX_ACTIVE = 4;

function integrationsSection(present: Record<string, boolean>): HTMLElement {
  const note = h("div", { class: "hint" });
  const list = h("div", { style: "display:flex;flex-direction:column;gap:14px" });

  function updateNote() {
    const used = settings.activeIntegrations.length;
    note.textContent = `Pick up to ${MAX_ACTIVE} pills to show next to Mochi — ${used}/${MAX_ACTIVE} in use. Keys are stored in the Windows Credential Manager, never on disk.`;
  }

  for (const def of INTEGRATIONS) {
    const active = settings.activeIntegrations.includes(def.id);
    const sw = h("button", { class: active ? "switch on" : "switch" });
    sw.addEventListener("click", () => {
      const on = settings.activeIntegrations.includes(def.id);
      if (on) {
        settings.activeIntegrations = settings.activeIntegrations.filter((x) => x !== def.id);
      } else {
        if (settings.activeIntegrations.length >= MAX_ACTIVE) return;
        settings.activeIntegrations = [...settings.activeIntegrations, def.id];
      }
      sw.classList.toggle("on", !on);
      updateNote();
      void save();
    });

    const rows = h("div", { style: "display:flex;flex-direction:column;gap:6px;flex:1 1 auto;min-width:0" });
    for (const field of def.fields) {
      const input = h("input", {
        type: field.secret ? "password" : "text",
        placeholder: present[field.key] ? "••••••••  (stored)" : field.placeholder,
        autocomplete: "off",
        spellcheck: "false",
        style: "flex:1 1 auto;min-width:0",
      }) as HTMLInputElement;
      const saveBtn = h("button", { text: "Save" });
      const dotEl = statusDot(present[field.key] ?? false);
      saveBtn.addEventListener("click", async () => {
        const value = input.value.trim();
        try {
          await Bridge.secretSet(field.key, value);
          present[field.key] = value.length > 0;
          input.value = "";
          input.placeholder = value ? "••••••••  (stored)" : field.placeholder;
          dotEl.style.background = value ? "#22c55e" : "#f4505e";
        } catch {
          dotEl.style.background = "#f5a524";
        }
      });
      rows.append(
        h("div", { class: "row" },
          h("label", { style: "min-width:104px", text: field.label }),
          input, saveBtn, dotEl,
        ),
      );
    }

    list.append(
      h("div", { style: "display:flex;gap:12px;align-items:flex-start" },
        h("div", { style: "display:flex;align-items:center;gap:8px;min-width:132px;padding-top:4px" },
          sw,
          h("i", { class: "dot", style: `background:${def.color}` }),
          h("span", { style: "font-size:12.5px", text: def.name }),
        ),
        rows,
      ),
    );
  }

  updateNote();
  return h("section", {}, h("h2", {}, h("span", { text: "Integrations" })), note, list);
}

// ── General section ───────────────────────────────────────────────────────────

function generalSection(): HTMLElement {
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    value: String(settings.soundVolume),
  }) as HTMLInputElement;
  volume.addEventListener("input", () => {
    settings.soundVolume = Number(volume.value);
    void save();
  });

  const autoClose = h("input", {
    type: "number", min: "5", max: "120", step: "1",
    value: String(Math.round(settings.autoCloseInterval)),
    style: "width:72px",
  }) as HTMLInputElement;
  autoClose.addEventListener("change", () => {
    settings.autoCloseInterval = Math.max(5, Math.min(120, Number(autoClose.value) || 15));
    autoClose.value = String(settings.autoCloseInterval);
    void save();
  });

  const screen = h("select", {}) as HTMLSelectElement;
  screen.append(
    h("option", { value: "primary", text: "Main display" }),
    h("option", { value: "cursor", text: "Display under the cursor" }),
  );
  screen.value = settings.screen;
  screen.addEventListener("change", () => {
    settings.screen = screen.value as Settings["screen"];
    void save();
  });

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "General" })),
    h("div", { class: "row" },
      h("label", { text: "Sound" }),
      toggle(settings.soundEnabled, (v) => { settings.soundEnabled = v; void save(); }),
      volume,
    ),
    h("div", { class: "row" },
      h("label", { text: "Auto-close" }),
      autoClose,
      h("span", { class: "hint", text: "seconds after you leave the island" }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Island lives on" }),
      screen,
    ),
    h("div", { class: "row" },
      h("label", { text: "Launch at startup" }),
      toggle(settings.autostart, (v) => { settings.autostart = v; void save(); }),
    ),
  );
}

// ── Boot ──────────────────────────────────────────────────────────────────────

async function drawAll() {
  const status = (await Bridge.hooksStatus()) ?? {
    installed: false, settingsPath: "", hookPath: "", hookReady: false,
  };

  const keys = [
    "stripe-api-key", "github-token", "vercel-token",
    "n8n-url", "n8n-api-key", "resend-api-key", "notion-api-key", "calcom-api-key",
    "anthropic-api-key", "google-api-key", "openai-api-key", "custom-api-key",
  ];
  const present: Record<string, boolean> = {};
  for (const k of keys) present[k] = (await Bridge.secretPresent(k)) ?? false;

  clear(root);
  root.append(
    h("h1", {}, h("span", { text: "Coucou" }), h("span", { class: "version", text: version })),
    claudeSection(status),
    await chatProvidersSection(),
    integrationsSection(present),
    generalSection(),
    h("div", {
      class: "hint",
      text: "No telemetry. Network requests only go to the services you configure yourself.",
    }),
  );
}

async function main() {
  const boot = await Bridge.boot();
  if (boot) {
    settings = { ...settings, ...boot.settings };
    version = boot.version;
  }

  await drawAll();

  void onEvent<Settings>("settings-changed", (s) => {
    settings = { ...settings, ...s };
  });
}

void main();
