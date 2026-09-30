"use strict";

const { meta, version, os, startup_when: startupWhen } = window.BOOT;
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

// ------------------------------------------------------------------ words

const PAGES = {
  performance: ["Performance", "DPI stages, polling rate and sensor."],
  lighting: ["Lighting", "The effect, color and brightness of the LED."],
  buttons: ["Buttons", "Choose what each button does."],
  power: ["Power", "Sleep timers and click debounce."],
};

const BUTTONS = [
  ["left", "Left button", "Main click"],
  ["right", "Right button", "Secondary click"],
  ["middle", "Wheel click", "Press the wheel"],
  ["wheel-up", "Scroll up", "Roll the wheel forward"],
  ["wheel-down", "Scroll down", "Roll the wheel back"],
  ["forward", "Forward", "Side, front"],
  ["back", "Back", "Side, rear"],
  ["dpi", "DPI button", "Underside"],
];

const ACTION_GROUPS = [
  ["Mouse", ["left-click", "right-click", "middle-click", "back", "forward", "double-click", "scroll-up", "scroll-down"]],
  ["DPI and polling", ["dpi-cycle", "dpi-up", "dpi-down", "polling-rate-cycle"]],
  ["Media", ["play-pause", "next-track", "previous-track", "stop", "mute", "volume-up", "volume-down", "media-player"]],
  ["Apps and browser", ["browser-back", "browser-forward", "browser-refresh", "browser-stop", "browser-home",
    "browser-search", "calculator", "email", "my-computer"]],
  ["Other", ["disabled"]],
];

const LABELS = {
  "dpi-cycle": "DPI cycle", "dpi-up": "DPI up", "dpi-down": "DPI down",
  "play-pause": "Play / pause", "my-computer": "This PC",
  "static-dpi": "Static DPI", "breathing-dpi": "Breathing DPI",
};
const human = (slug) => LABELS[slug] ?? slug.charAt(0).toUpperCase() + slug.slice(1).replace(/-/g, " ");
const sentence = (text) => text.charAt(0).toUpperCase() + text.slice(1);

const KEY_LABELS = {
  ctrl: "Ctrl", shift: "Shift", alt: os === "macos" ? "Option" : "Alt",
  win: { macos: "Cmd", linux: "Super" }[os] ?? "Win", esc: "Esc", enter: "Enter", backspace: "Backspace",
  tab: "Tab", space: "Space", minus: "-", equal: "=", leftbracket: "[", rightbracket: "]", backslash: "\\",
  nonushash: "#", semicolon: ";", quote: "'", grave: "`", comma: ",", period: ".", slash: "/",
  capslock: "Caps Lock", printscreen: "Print Screen", scrolllock: "Scroll Lock", pause: "Pause", insert: "Insert",
  home: "Home", pageup: "Page Up", delete: "Delete", end: "End", pagedown: "Page Down",
  right: "→", left: "←", down: "↓", up: "↑",
};
const keyLabel = (key) => KEY_LABELS[key] ?? key.toUpperCase();

// KeyboardEvent.code -> the driver's key names (USB HID usages).
const CODE_TO_KEY = (() => {
  const map = {
    Enter: "enter", NumpadEnter: "enter", Escape: "esc", Backspace: "backspace", Tab: "tab", Space: "space",
    Minus: "minus", Equal: "equal", BracketLeft: "leftbracket", BracketRight: "rightbracket",
    Backslash: "backslash", Semicolon: "semicolon", Quote: "quote", Backquote: "grave", Comma: "comma",
    Period: "period", Slash: "slash", CapsLock: "capslock", PrintScreen: "printscreen", ScrollLock: "scrolllock",
    Pause: "pause", Insert: "insert", Home: "home", PageUp: "pageup", Delete: "delete", End: "end",
    PageDown: "pagedown", ArrowRight: "right", ArrowLeft: "left", ArrowDown: "down", ArrowUp: "up",
  };
  for (let c = 0; c < 26; c++) map["Key" + String.fromCharCode(65 + c)] = String.fromCharCode(97 + c);
  for (let d = 0; d <= 9; d++) map["Digit" + d] = String(d);
  for (let f = 1; f <= 12; f++) map["F" + f] = "f" + f;
  return map;
})();
const MODIFIER_CODES = new Set([
  "ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight",
]);

const ICONS = {
  ok: '<svg viewBox="0 0 14 14"><path d="M2.5 7.5l3 3 6-7"/></svg>',
  error: '<svg viewBox="0 0 14 14"><path d="M7 3v5M7 10.8v.2"/></svg>',
};

// ------------------------------------------------------------- DPI scale

// The slider is logarithmic so low values, where most people live, get room.
const DPI_MIN = meta.dpi.min;
const DPI_MAX = meta.dpi.max;
const LOG_MIN = Math.log(DPI_MIN);
const LOG_SPAN = Math.log(DPI_MAX) - LOG_MIN;
const dpiToPos = (dpi) => Math.round(((Math.log(dpi) - LOG_MIN) / LOG_SPAN) * 1000);
const posToDpi = (pos) => snapDpi(Math.exp(LOG_MIN + (pos / 1000) * LOG_SPAN));

// Supported values: steps of 50 up to 10000, 100 up to 20000, 200 above.
function snapDpi(value) {
  const clamped = Math.min(DPI_MAX, Math.max(DPI_MIN, value));
  const step = clamped <= 10000 ? 50 : clamped <= 20000 ? 100 : 200;
  return Math.min(DPI_MAX, Math.max(DPI_MIN, Math.round(clamped / step) * step));
}
const dpiStep = (dpi) => (dpi < 10000 ? 50 : dpi < 20000 ? 100 : 200);

// ------------------------------------------------------------------- API

class ApiError extends Error {
  constructor(detail) {
    super(detail?.error ?? String(detail));
    this.reason = detail?.reason;  // disconnected, asleep, device or invalid
  }
}

async function call(command, args) {
  try {
    return await invoke(command, args);
  } catch (detail) {
    throw new ApiError(detail);
  }
}

// ----------------------------------------------------------------- state

let state = null;          // settings of the active profile, from get_state
let editStage = null;      // DPI stage shown in the editor; the active one at first
let recording = null;      // button whose shortcut is being recorded
let queue = Promise.resolve();  // device calls run one at a time, in order
let loading = false;
let seen = { stage: null, profile: null };

function enqueue(task) {
  queue = queue.then(task, task);
  return queue;
}

function save(command, patch) {
  return enqueue(async () => {
    toast("Saving", "busy");
    try {
      Object.assign(state, await call(command, { patch }));
      toast("Saved", "ok");
    } catch (error) {
      reportError(error);
    }
    render();
  });
}

// Called at start and on each live update while there is no state yet, which
// also makes it retry after a failure.
function loadState() {
  if (loading) return;
  loading = true;
  enqueue(async () => {
    try {
      const first = state === null && editStage === null;
      state = await call("get_state");
      if (first) editStage = state.dpi.active;
      overlay(null);
      render();
    } catch (error) {
      state = null;
      overlay(error.reason === "disconnected" ? "disconnected" : error.reason === "asleep" ? "asleep" : "error",
        error.message);
    } finally {
      loading = false;
    }
  });
}

function reportError(error) {
  if (error.reason === "disconnected") {
    state = null;
    overlay("disconnected");
  } else if (error.reason === "asleep") {
    toast("The mouse didn't answer. Move it to wake it up, then try again.", "error");
  } else {
    toast(sentence(error.message || "Something went wrong."), "error");
  }
}

// ------------------------------------------------------------ live status

// The app pushes a live snapshot on every change, and every 2 s at least.
function onLive(live) {
  renderLive(live);
  if (!live.connected && state) {
    state = null;
    overlay("disconnected");
  }
  if (live.connected && !state) loadState();
  if (state && seen.stage !== null && live.stage.seq !== seen.stage && live.stage.value) {
    state.dpi.active = live.stage.value;  // someone pressed the DPI button
    renderPerformance();
  }
  if (state && seen.profile !== null && live.profile.seq !== seen.profile) loadState();
  seen = { stage: live.stage.seq, profile: live.profile.seq };
}

async function followLive() {
  await listen("live", (event) => onLive(event.payload));
  onLive(await call("get_live"));
}

function renderLive(live) {
  const link = $("#link");
  const fresh = live.battery && live.battery.age < 10;
  link.className = "link " + (live.connected ? (fresh ? "ok" : "idle") : "");
  const via = live.connection === "USB cable" ? "cable" : "2.4 GHz";
  $("#link-text").textContent = !live.connected
    ? "Dongle not found"
    : fresh ? `Connected · ${via}` : live.battery ? "Mouse off or out of range" : "Dongle connected";

  const battery = live.battery;
  const level = battery ? battery.level : null;
  $("#battery-level").textContent = level ?? "––";
  $("#battery").classList.toggle("charging", battery?.state === "charging");
  const lit = level === null ? 0 : Math.ceil(level / 10);
  $$("#battery-meter i").forEach((segment, i) => {
    segment.classList.toggle("on", i < lit);
    segment.classList.toggle("next", i === Math.min(lit, 9));
  });
  $("#battery-state").textContent = !battery
    ? (live.connected ? "Waiting for a report" : "Waiting for the mouse")
    : { discharging: "Discharging", charging: "Charging", full: "Fully charged" }[battery.state] ?? "Unknown state";

  // null where starting with the system isn't supported
  $("#startup-row").hidden = live.startup === null || live.startup === undefined;
  $("#startup").setAttribute("aria-checked", String(Boolean(live.startup)));
}

async function toggleStartup() {
  const button = $("#startup");
  const enabled = button.getAttribute("aria-checked") !== "true";
  button.setAttribute("aria-checked", String(enabled));
  try {
    const result = await call("set_startup", { enabled });
    button.setAttribute("aria-checked", String(result.startup));
    toast(result.startup ? `Starts ${startupWhen}` : `Won't start ${startupWhen}`, "ok");
  } catch (error) {
    button.setAttribute("aria-checked", String(!enabled));
    reportError(error);
  }
}

// ----------------------------------------------------------------- render

function render() {
  if (!state) return;
  renderProfiles();
  renderPerformance();
  renderLighting();
  renderButtons();
  renderPower();
}

function setRangeFill(input) {
  const pct = ((input.value - input.min) / (input.max - input.min)) * 100;
  input.style.setProperty("--pct", pct + "%");
}

function setPressed(buttons, isPressed) {
  buttons.forEach((button) => button.setAttribute("aria-pressed", String(isPressed(button))));
}

function renderProfiles() {
  const box = $("#profiles");
  if (box.children.length !== state.profile.count) {
    box.innerHTML = "";
    for (let n = 1; n <= state.profile.count; n++) {
      const pill = document.createElement("button");
      pill.className = "profile-pill";
      pill.textContent = n;
      pill.dataset.profile = n;
      pill.title = `Switch to profile ${n}`;
      box.append(pill);
    }
  }
  setPressed($$(".profile-pill"), (pill) => Number(pill.dataset.profile) === state.profile.active);
}

function renderPerformance() {
  const dpi = state.dpi;
  $("#stage-count").textContent = dpi.count;
  $$(".stepper .step").forEach((step) => {
    const next = dpi.count + Number(step.dataset.step);
    step.disabled = next < 1 || next > meta.max_stages;
  });

  $$(".stage").forEach((tile) => {
    const n = Number(tile.dataset.stage);
    const active = n === dpi.active;
    const disabled = n > dpi.count;
    tile.classList.toggle("active", active);
    tile.classList.toggle("selected", n === editStage);
    tile.classList.toggle("disabled", disabled);
    tile.setAttribute("aria-pressed", String(n === editStage));
    tile.style.setProperty("--led", dpi.colors[n - 1]);
    $(".stage-dpi", tile).textContent = dpi.stages[n - 1];
    $(".stage-tag", tile).textContent = active ? "Active" : disabled ? "Off" : "";
  });

  const value = dpi.stages[editStage - 1];
  $("#edit-stage").textContent = editStage;
  if (document.activeElement !== $("#dpi-input")) $("#dpi-input").value = value;
  const slider = $("#dpi-slider");
  slider.value = dpiToPos(value);
  setRangeFill(slider);
  $("#stage-color").value = dpi.colors[editStage - 1];
  $("#stage-color").closest(".swatch").style.setProperty("--chip", dpi.colors[editStage - 1]);
  const makeActive = $("#make-active");
  makeActive.disabled = editStage === dpi.active || editStage > dpi.count;
  makeActive.textContent = editStage === dpi.active ? "Active stage" : "Make active";

  setPressed($$("#polling button"), (button) => Number(button.dataset.hz) === state.polling_rate);
  $("#polling-interval").textContent = state.polling_rate ? `${1000 / state.polling_rate} ms` : "–";
  $("#angle-snap").setAttribute("aria-checked", String(dpi.angle_snap));
  $("#ripple").setAttribute("aria-checked", String(dpi.ripple));
}

function renderLighting() {
  const light = state.lighting;
  setPressed($$(".mode"), (tile) => tile.dataset.mode === light.mode);
  $("#light-color").value = light.color;
  $("#light-color-hex").textContent = light.color;
  $("#ctl-color .swatch").style.setProperty("--chip", light.color);
  setPressed($$("#speed button"), (button) => Number(button.dataset.speed) === light.speed);
  $("#brightness").value = light.brightness;
  $("#brightness-value").textContent = light.brightness;
  setRangeFill($("#brightness"));

  const mode = light.mode;
  $("#ctl-color").classList.toggle("inactive", !["static", "breathing"].includes(mode));
  $("#ctl-speed").classList.toggle("inactive", !["breathing", "neon", "color-breathing", "breathing-dpi"].includes(mode));
  $("#ctl-brightness").classList.toggle("inactive", mode === "off");
  $("#dpi-color-note").classList.toggle("inactive", !["static-dpi", "breathing-dpi"].includes(mode));
}

function renderKeys(element, combo) {
  const keys = combo.slice(4).split("+");
  element.innerHTML = keys
    .map((key) => `<kbd>${escapeHtml(keyLabel(key))}</kbd>`)
    .join('<span class="plus">+</span>');
}

function renderButtons() {
  for (const [name] of BUTTONS) {
    if (recording === name) continue;
    const row = $(`.binding[data-button="${name}"]`);
    const select = $("select", row);
    const keys = $(".keys", row);
    const binding = state.buttons[name];
    $$("option[data-current]", select).forEach((option) => option.remove());
    if (binding.startsWith("key:")) {
      select.value = "__key__";
      keys.hidden = false;
      keys.classList.remove("recording");
      renderKeys(keys, binding);
    } else {
      keys.hidden = true;
      if (![...select.options].some((option) => option.value === binding)) {
        // Set by other software to something this interface doesn't offer (a macro, rapid fire...).
        const option = new Option(`${human(binding)} (current)`, binding);
        option.dataset.current = "";
        option.disabled = true;
        select.prepend(option);
      }
      select.value = binding;
    }
  }
}

function formatMinutes(minutes) {
  return Number.isInteger(minutes) ? String(minutes) : minutes.toFixed(1);
}

function renderPower() {
  const light = state.lighting;
  const values = { sleep: light.sleep * 2, "deep-sleep": light.deep_sleep, debounce: light.debounce };
  for (const [id, value] of Object.entries(values)) {
    const input = $("#" + id);
    input.value = value;
    setRangeFill(input);
  }
  $("#sleep-value").textContent = formatMinutes(light.sleep);
  $("#deep-sleep-value").textContent = light.deep_sleep;
  $("#debounce-value").textContent = light.debounce;
}

// --------------------------------------------------------------- building

function escapeHtml(text) {
  return text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function build() {
  $("#battery-meter").innerHTML = "<i></i>".repeat(10);

  $("#stages").innerHTML = Array.from({ length: meta.max_stages }, (_, i) => `
    <button class="stage" data-stage="${i + 1}">
      <span class="stage-top"><span class="stage-no">${String(i + 1).padStart(2, "0")}</span><span class="stage-led"></span></span>
      <span class="stage-dpi">–</span>
      <span class="stage-tag"></span>
    </button>`).join("");

  $("#dpi-ticks").innerHTML = [50, 400, 1600, 6400, 22000]
    .map((dpi) => `<span style="left:${dpiToPos(dpi) / 10}%">${dpi >= 1000 ? dpi / 1000 + "k" : dpi}</span>`)
    .join("");

  $("#polling").innerHTML = meta.polling_rates
    .map((hz) => `<button data-hz="${hz}">${hz}<small>Hz</small></button>`).join("");

  $("#modes").innerHTML = meta.light_modes
    .map((mode) => `<button class="mode" data-mode="${mode}"><span class="orb orb-${mode}"></span><span class="mode-name">${human(mode)}</span></button>`)
    .join("");

  $("#speed").innerHTML = [1, 2, 3, 4, 5].map((n) => `<button data-speed="${n}">${n}</button>`).join("");

  const offered = new Set(ACTION_GROUPS.flatMap(([, actions]) => actions));
  const groups = ACTION_GROUPS.map(([label, actions]) => [label, actions.filter((a) => meta.actions.includes(a))]);
  groups[groups.length - 1][1].push(...meta.actions.filter((a) => !offered.has(a)));
  const options = groups
    .map(([label, actions]) => `<optgroup label="${label}">${actions.map((a) => `<option value="${a}">${human(a)}</option>`).join("")}</optgroup>`)
    .join("") + '<optgroup label="Keyboard"><option value="__key__">Keyboard shortcut…</option></optgroup>';

  $("#bindings").innerHTML = BUTTONS.map(([name, title, where]) => `
    <div class="binding" data-button="${name}">
      <div><div class="binding-name">${title}</div><div class="binding-where">${where}</div></div>
      <div class="binding-control">
        <div class="select"><select aria-label="${title}">${options}</select></div>
        <button class="keys" hidden title="Click to record a new shortcut"></button>
      </div>
    </div>`).join("");
}

// ----------------------------------------------------------------- events

function setPage(page) {
  if (!PAGES[page]) page = "performance";
  $$(".nav-item").forEach((item) => {
    if (item.dataset.page === page) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  });
  $$(".page").forEach((section) => section.classList.toggle("active", section.dataset.page === page));
  const [title, sub] = PAGES[page];
  $("#page-title").textContent = title;
  $("#page-sub").textContent = sub;
  window.scrollTo(0, 0);
  try { localStorage.setItem("page", page); } catch { /* storage unavailable */ }
}

function commitDpi(raw) {
  const parsed = parseInt(String(raw).replace(/[^\d]/g, ""), 10);
  if (!state) return;
  if (Number.isNaN(parsed)) return renderPerformance();
  const value = snapDpi(parsed);
  if (value === state.dpi.stages[editStage - 1]) return renderPerformance();
  const stages = [...state.dpi.stages];
  stages[editStage - 1] = value;
  save("update_dpi", { stages });
}

function setHot(name) {
  $$(".mouse .part").forEach((part) => part.classList.toggle("hot", part.dataset.button === name));
  $$(".binding").forEach((row) => row.classList.toggle("hot", row.dataset.button === name));
}

function startRecording(name) {
  stopRecording();
  recording = name;
  const keys = $(`.binding[data-button="${name}"] .keys`);
  keys.hidden = false;
  keys.classList.add("recording");
  keys.innerHTML = 'Press a shortcut<span class="caret"></span>';
  keys.focus();
  window.addEventListener("keydown", onRecordKey, true);
}

function stopRecording() {
  if (!recording) return;
  window.removeEventListener("keydown", onRecordKey, true);
  recording = null;
  render();
}

function onRecordKey(event) {
  event.preventDefault();
  event.stopPropagation();
  const mods = [event.ctrlKey && "ctrl", event.shiftKey && "shift", event.altKey && "alt", event.metaKey && "win"]
    .filter(Boolean);
  const keys = $(`.binding[data-button="${recording}"] .keys`);
  if (MODIFIER_CODES.has(event.code)) {
    keys.innerHTML = mods.map((m) => `<kbd>${keyLabel(m)}</kbd>`).join('<span class="plus">+</span>')
      + '<span class="plus">+</span><span class="caret"></span>';
    return;
  }
  if (event.code === "Escape" && mods.length === 0) return stopRecording();
  const key = CODE_TO_KEY[event.code];
  if (!key || !meta.keys.includes(key)) {
    toast(`${event.key} can't be assigned to a button.`, "error");
    return;
  }
  const name = recording;
  stopRecording();
  save("update_buttons", { bindings: { [name]: "key:" + [...mods, key].join("+") } });
}

function wire() {
  $$(".nav-item").forEach((item) => item.addEventListener("click", () => setPage(item.dataset.page)));

  $("#profiles").addEventListener("click", (event) => {
    const pill = event.target.closest(".profile-pill");
    if (pill && state && Number(pill.dataset.profile) !== state.profile.active) {
      save("update_profile", { active: Number(pill.dataset.profile) });
    }
  });

  // Performance
  $("#stages").addEventListener("click", (event) => {
    const tile = event.target.closest(".stage");
    if (!tile || !state) return;
    editStage = Number(tile.dataset.stage);
    renderPerformance();
  });
  $("#stages").addEventListener("dblclick", (event) => {
    const tile = event.target.closest(".stage");
    if (tile && state && Number(tile.dataset.stage) <= state.dpi.count) save("update_dpi", { active: Number(tile.dataset.stage) });
  });
  $$(".stepper .step").forEach((step) => step.addEventListener("click", () => {
    if (state) save("update_dpi", { count: state.dpi.count + Number(step.dataset.step) });
  }));

  const input = $("#dpi-input");
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") input.blur();
    if (event.key === "Escape") { input.value = state.dpi.stages[editStage - 1]; input.blur(); }
    if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      event.preventDefault();
      const current = snapDpi(parseInt(input.value, 10) || state.dpi.stages[editStage - 1]);
      const next = snapDpi(current + (event.key === "ArrowUp" ? dpiStep(current) : -dpiStep(current - 1)));
      input.value = next;
      $("#dpi-slider").value = dpiToPos(next);
      setRangeFill($("#dpi-slider"));
    }
  });
  input.addEventListener("focus", () => input.select());
  input.addEventListener("blur", () => commitDpi(input.value));

  const slider = $("#dpi-slider");
  slider.addEventListener("input", () => {
    input.value = posToDpi(Number(slider.value));
    setRangeFill(slider);
  });
  slider.addEventListener("change", () => commitDpi(posToDpi(Number(slider.value))));

  const stageColor = $("#stage-color");
  stageColor.addEventListener("input", () => {
    stageColor.closest(".swatch").style.setProperty("--chip", stageColor.value);
    $(`.stage[data-stage="${editStage}"]`).style.setProperty("--led", stageColor.value);
  });
  stageColor.addEventListener("change", () => {
    const colors = [...state.dpi.colors];
    colors[editStage - 1] = stageColor.value;
    save("update_dpi", { colors });
  });
  $("#make-active").addEventListener("click", () => save("update_dpi", { active: editStage }));

  $("#polling").addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (button && state && Number(button.dataset.hz) !== state.polling_rate) {
      save("update_polling_rate", { hz: Number(button.dataset.hz) });
    }
  });
  $("#angle-snap").addEventListener("click", () => state && save("update_dpi", { angle_snap: !state.dpi.angle_snap }));
  $("#ripple").addEventListener("click", () => state && save("update_dpi", { ripple: !state.dpi.ripple }));

  // Lighting
  $("#modes").addEventListener("click", (event) => {
    const tile = event.target.closest(".mode");
    if (tile && state && tile.dataset.mode !== state.lighting.mode) save("update_lighting", { mode: tile.dataset.mode });
  });
  const lightColor = $("#light-color");
  lightColor.addEventListener("input", () => {
    $("#ctl-color .swatch").style.setProperty("--chip", lightColor.value);
    $("#light-color-hex").textContent = lightColor.value;
  });
  lightColor.addEventListener("change", () => save("update_lighting", { color: lightColor.value }));
  $("#speed").addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (button && state) save("update_lighting", { speed: Number(button.dataset.speed) });
  });
  const brightness = $("#brightness");
  brightness.addEventListener("input", () => {
    $("#brightness-value").textContent = brightness.value;
    setRangeFill(brightness);
  });
  brightness.addEventListener("change", () => save("update_lighting", { brightness: Number(brightness.value) }));

  // Buttons
  $("#bindings").addEventListener("change", (event) => {
    const select = event.target.closest("select");
    if (!select || !state) return;
    const name = select.closest(".binding").dataset.button;
    if (select.value === "__key__") startRecording(name);
    else save("update_buttons", { bindings: { [name]: select.value } });
  });
  $("#bindings").addEventListener("click", (event) => {
    const keys = event.target.closest(".keys");
    if (keys && recording !== keys.closest(".binding").dataset.button) startRecording(keys.closest(".binding").dataset.button);
  });
  $("#bindings").addEventListener("focusout", (event) => {
    if (recording && event.target.closest(".keys")) stopRecording();
  });
  $("#bindings").addEventListener("mouseover", (event) => setHot(event.target.closest(".binding")?.dataset.button ?? null));
  $("#bindings").addEventListener("mouseleave", () => setHot(null));
  $("#mouse").addEventListener("mouseover", (event) => setHot(event.target.closest(".part")?.dataset.button ?? null));
  $("#mouse").addEventListener("mouseleave", () => setHot(null));
  $("#mouse").addEventListener("click", (event) => {
    const part = event.target.closest(".part");
    if (part) $(`.binding[data-button="${part.dataset.button}"] select`).focus();
  });
  $("#buttons-reset").addEventListener("click", () => state && save("update_buttons", { reset: true }));

  // Power
  const power = {
    sleep: [(v) => formatMinutes(v / 2), (v) => ({ sleep: v / 2 })],
    "deep-sleep": [(v) => String(v), (v) => ({ deep_sleep: v })],
    debounce: [(v) => String(v), (v) => ({ debounce: v })],
  };
  for (const [id, [show, body]] of Object.entries(power)) {
    const range = $("#" + id);
    range.addEventListener("input", () => {
      $(`#${id}-value`).textContent = show(Number(range.value));
      setRangeFill(range);
    });
    range.addEventListener("change", () => save("update_lighting", body(Number(range.value))));
  }

  $("#startup").addEventListener("click", toggleStartup);

  $("#theme-toggle").addEventListener("click", () => {
    const order = ["", "light", "dark"];
    const next = order[(order.indexOf(document.documentElement.dataset.theme ?? "") + 1) % order.length];
    applyTheme(next);
    try { localStorage.setItem("theme", next); } catch { /* storage unavailable */ }
  });
}

function applyTheme(theme) {
  if (theme) document.documentElement.dataset.theme = theme;
  else delete document.documentElement.dataset.theme;
  $("#theme-toggle").title = `Theme: ${theme || "system"}`;
}

// ----------------------------------------------------------- toast, overlay

let toastTimer = null;

function toast(text, kind) {
  const element = $("#toast");
  element.className = `toast show ${kind}`;
  element.innerHTML = kind === "busy" ? '<span class="spinner"></span>' : ICONS[kind];
  element.append(document.createTextNode(text));
  clearTimeout(toastTimer);
  if (kind !== "busy") toastTimer = setTimeout(() => element.classList.remove("show"), kind === "error" ? 6000 : 1500);
}

const OVERLAYS = {
  connecting: ["Connecting", "Looking for the mouse."],
  disconnected: ["Dongle not found", "Plug in the 2.4 GHz dongle, or connect the mouse with its cable."],
  asleep: ["The mouse isn't answering", "Move it to wake it up. This retries on its own."],
  error: ["Something went wrong", ""],
};

function overlay(kind, detail) {
  const element = $("#overlay");
  if (!kind) {
    element.hidden = true;
    return;
  }
  const [title, text] = OVERLAYS[kind];
  $("#overlay-title").textContent = title;
  $("#overlay-text").textContent = kind === "error" ? sentence(detail || "") : text;
  element.hidden = false;
}

// ------------------------------------------------------------------ start

let savedTheme = "";
let savedPage = "performance";
try {
  savedTheme = localStorage.getItem("theme") || "";
  savedPage = localStorage.getItem("page") || "performance";
} catch { /* storage unavailable */ }

build();
wire();
applyTheme(savedTheme);
setPage(location.hash.slice(1) || savedPage);
$("#version").textContent = `v${version}`;
$("#startup-label").textContent = `Start ${startupWhen}`;
$("#startup").setAttribute("aria-label", `Start ${startupWhen}`);
overlay("connecting");
loadState();
followLive();
