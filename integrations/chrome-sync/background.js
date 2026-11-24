const NATIVE_HOST_NAME = "io.github.wuxm5.authbaton.chrome";
const CLAUDE_EXT_ID = "fcoeoabgfenejglbffodgkkbkcdhcgfn";
const CLAUDE_EXT_CLIENT_ID = "dae2cad8-15c5-43d2-9046-fcaecc135fa4";
const ARM_KEY = "authbaton_oauth_arm_v1";
const STATUS_KEY = "authbaton_popup_status_v1";
const KEEPALIVE_ALARM = "authbaton-native-keepalive";
const EXTENSION_VERSION = chrome.runtime.getManifest().version || "0.1.0";

// Browser-level and Cloudflare cookies must stay untouched across switches.
const EXCLUDED_COOKIE_PATTERNS = [
  /^cf_clearance$/i,
  /^__cf_bm$/i,
  /^__cflb$/i,
  /^_cfuvid$/i,
  /^cf_chl_/i,
  /^anthropic-device-id$/i,
  /^ajs_/i,
  /^_ga/i,
  /^_gid$/i,
  /^_fbp$/i,
  /^__stripe_/i,
  /^_dd_/i,
];

// Cookies persisted in AuthBaton Vault and swapped per account.
const AUTH_COOKIE_EXACT = new Set([
  "sessionKey",
  "sessionKeyV3",
  "sessionKeyLC",
  "sessionKeyV3LC",
  "lastActiveOrg",
  "routingHint",
  "__Host-sessionBinding",
  "__Host-sessionBindingGrace",
]);

// Per-account transient cookies cleared on switch so old account hints never leak.
const TRANSIENT_CLEAR_PATTERNS = [
  /^__Host-sessionBindingMiss$/i,
  /^intercom-session-/i,
];

function isAuthCookie(name) {
  if (EXCLUDED_COOKIE_PATTERNS.some((re) => re.test(name))) return false;
  if (AUTH_COOKIE_EXACT.has(name)) return true;
  return /^(sessionKey|routingHint|lastActiveOrg|__Secure-.*session|claude_session)/i.test(name);
}

function shouldRemoveOnSwitch(name) {
  if (isAuthCookie(name)) return true;
  return TRANSIENT_CLEAR_PATTERNS.some((re) => re.test(name));
}

function isEphemeralBindingCookie(name) {
  return name === "__Host-sessionBinding" || name === "__Host-sessionBindingGrace";
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function cookieUrl(c) {
  const scheme = c.secure ? "https" : "http";
  const host = (c.domain || "claude.ai").replace(/^\./, "");
  const path = c.path || "/";
  return `${scheme}://${host}${path}`;
}

function serializeCookie(c) {
  return {
    name: c.name,
    value: c.value,
    domain: c.domain,
    hostOnly: !!c.hostOnly,
    path: c.path || "/",
    secure: !!c.secure,
    httpOnly: !!c.httpOnly,
    sameSite: c.sameSite || "unspecified",
    session: !!c.session,
    expirationDate: typeof c.expirationDate === "number" ? c.expirationDate : undefined,
    storeId: c.storeId,
  };
}

async function getAllClaudeCookiesRaw() {
  const [byUrl, byDomain] = await Promise.all([
    chrome.cookies.getAll({ url: "https://claude.ai" }),
    chrome.cookies.getAll({ domain: "claude.ai" }),
  ]);
  const dedup = new Map();
  for (const c of [...byUrl, ...byDomain]) {
    const key = `${c.storeId || ""}|${c.domain}|${c.path}|${c.name}`;
    dedup.set(key, c);
  }
  return Array.from(dedup.values());
}

async function setCookieSafe(c) {
  const details = {
    url: cookieUrl(c),
    name: c.name,
    value: c.value,
    path: c.path || "/",
    secure: !!c.secure,
    httpOnly: !!c.httpOnly,
    sameSite: c.sameSite || "unspecified",
  };
  if (!c.hostOnly && c.domain && !c.name.startsWith("__Host-")) {
    details.domain = c.domain;
  }
  if (!c.session && typeof c.expirationDate === "number") {
    details.expirationDate = c.expirationDate;
  }
  if (c.storeId) {
    details.storeId = c.storeId;
  }
  if (details.sameSite === "no_restriction") {
    details.secure = true;
  }
  return chrome.cookies.set(details);
}

async function probeBootstrap() {
  try {
    const resp = await fetch("https://claude.ai/api/bootstrap", {
      method: "GET",
      credentials: "include",
      cache: "no-store",
      redirect: "manual",
    });
    const cfMitigated = resp.headers.get("cf-mitigated");
    if (resp.status === 401 || resp.status === 403) {
      let errorCode = null;
      try {
        const body = await resp.json();
        errorCode = body?.error?.details?.error_code || body?.error?.type || null;
      } catch {}
      return {
        ok: false,
        signedIn: false,
        status: resp.status,
        cfMitigated,
        errorCode,
        deviceBindingLost: errorCode === "session_device_binding_lost",
      };
    }
    if (!resp.ok) {
      return { ok: false, signedIn: null, status: resp.status, cfMitigated };
    }
    const body = await resp.json();
    const account = body?.account || null;
    if (!account || !account.uuid) {
      return { ok: true, signedIn: false, status: resp.status };
    }
    const memberships = Array.isArray(account.memberships) ? account.memberships : [];
    const orgs = memberships
      .map((m) => m?.organization)
      .filter((o) => o && typeof o.uuid === "string")
      .map((o) => ({
        uuid: o.uuid,
        name: o.name || o.uuid,
      }));
    return {
      ok: true,
      signedIn: true,
      status: resp.status,
      accountUuid: account.uuid,
      email: account.email_address || "",
      orgs,
    };
  } catch (err) {
    return { ok: false, signedIn: null, error: String(err) };
  }
}

async function getClaudeExtensionStatus() {
  try {
    const info = await chrome.management.get(CLAUDE_EXT_ID);
    return {
      installed: true,
      version: info.version || "",
      enabled: !!info.enabled,
      may_disable: !!info.mayDisable,
    };
  } catch {
    return {
      installed: false,
      version: "",
      enabled: false,
      may_disable: false,
    };
  }
}

async function resetClaudeExtension() {
  const before = await getClaudeExtensionStatus();
  if (!before.installed) {
    return { ok: false, skipped: true, installed: false, reason: "not_installed" };
  }
  if (!before.may_disable) {
    return { ok: false, skipped: false, installed: true, reason: "may_disable_false" };
  }
  await chrome.management.setEnabled(CLAUDE_EXT_ID, false);
  await sleep(450);
  await chrome.management.setEnabled(CLAUDE_EXT_ID, true);
  await sleep(300);
  const after = await getClaudeExtensionStatus();
  return { ok: !!after.enabled, skipped: false, installed: true, version: after.version };
}

async function armOAuthAutoApprove({
  targetAccountUuid,
  targetOrgUuid,
  targetOrgName,
  autoCloseInstalledTab = true,
  ttlMs = 90000,
}) {
  const state = {
    armedAt: Date.now(),
    armedUntil: Date.now() + ttlMs,
    clientId: CLAUDE_EXT_CLIENT_ID,
    targetAccountUuid: targetAccountUuid || "",
    targetOrgUuid: targetOrgUuid || "",
    targetOrgName: targetOrgName || "",
    autoCloseInstalledTab: !!autoCloseInstalledTab,
    clickAttempted: false,
  };
  await chrome.storage.session.set({ [ARM_KEY]: state });
  return state;
}

async function getOAuthArmState() {
  const data = await chrome.storage.session.get(ARM_KEY).catch(() => ({}));
  const state = data[ARM_KEY];
  if (!state) return { armed: false };
  if (Date.now() > state.armedUntil) {
    await chrome.storage.session.remove(ARM_KEY).catch(() => {});
    return { armed: false, expired: true };
  }
  return { armed: true, ...state };
}

async function disarmOAuth() {
  await chrome.storage.session.remove(ARM_KEY).catch(() => {});
}

// Persistent Native Messaging connection to AuthBaton.
let nativePort = null;
let reconnectTimer = null;
let reconnectDelayMs = 2000;
let suppressCookieEventsUntil = 0;
let cookieChangeTimer = null;
let lastObservedStatus = {
  connected: false,
  lastError: null,
  signedIn: false,
  email: "",
  claudeExt: { installed: false, enabled: false, version: "", may_disable: false },
};

async function savePopupStatus(patch) {
  lastObservedStatus = { ...lastObservedStatus, ...patch, updatedAt: Date.now() };
  await chrome.storage.session.set({ [STATUS_KEY]: lastObservedStatus }).catch(() => {});
}

function postNative(payload) {
  if (!nativePort) return false;
  try {
    nativePort.postMessage(payload);
    return true;
  } catch (err) {
    disconnectNative(String(err));
    return false;
  }
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  const delay = reconnectDelayMs;
  reconnectDelayMs = Math.min(reconnectDelayMs * 2, 15000);
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connectNative();
  }, delay);
}

function disconnectNative(reason) {
  if (nativePort) {
    try {
      nativePort.disconnect();
    } catch {}
    nativePort = null;
  }
  savePopupStatus({ connected: false, lastError: reason || null });
  scheduleReconnect();
}

async function captureAndReportState({ includeCookies = true, requestId = null, reason = "state" } = {}) {
  const [bootstrap, claudeExt, rawCookies] = await Promise.all([
    probeBootstrap(),
    getClaudeExtensionStatus(),
    getAllClaudeCookiesRaw(),
  ]);
  const authCookies = rawCookies.filter((c) => isAuthCookie(c.name)).map(serializeCookie);
  const hasSessionKey = authCookies.some((c) => c.name === "sessionKey" || c.name === "sessionKeyV3");

  await savePopupStatus({
    signedIn: !!bootstrap.signedIn,
    email: bootstrap.email || "",
    claudeExt,
  });

  const msg = {
    type: reason === "capture" ? "capture_result" : "state",
    request_id: requestId,
    extension_version: EXTENSION_VERSION,
    signed_in: !!bootstrap.signedIn,
    account_uuid: bootstrap.accountUuid || null,
    email: bootstrap.email || null,
    orgs: bootstrap.orgs || [],
    claude_ext: claudeExt,
    device_binding_lost: !!bootstrap.deviceBindingLost,
    cf_mitigated: bootstrap.cfMitigated || null,
    cookies: includeCookies && bootstrap.signedIn && hasSessionKey ? authCookies : null,
  };
  postNative(msg);
  return { ok: !!bootstrap.signedIn && hasSessionKey, bootstrap, claudeExt, hasSessionKey };
}

async function handleSwitchAccount(msg) {
  const requestId = msg.request_id || "";
  const targetAccountUuid = msg.target_account_uuid || "";
  const targetOrgUuid = msg.target_org_uuid || "";
  const targetOrgName = msg.target_org_name || "";
  const targetCookies = Array.isArray(msg.cookies) ? msg.cookies : [];
  const resetExt = msg.reset_claude_ext !== false;
  const armOAuth = msg.arm_oauth !== false;
  const autoClose = msg.auto_close_installed_tab !== false;

  if (!targetAccountUuid || targetCookies.length === 0) {
    postNative({
      type: "switch_result",
      request_id: requestId,
      ok: false,
      error: "目标账号缺少已保存的网页会话",
    });
    return;
  }

  suppressCookieEventsUntil = Date.now() + 8000;
  try {
    // 1. Snapshot currently active account first if it differs from target so rotated cookies are kept.
    const before = await probeBootstrap();
    if (before.signedIn && before.accountUuid && before.accountUuid !== targetAccountUuid) {
      const currentRaw = await getAllClaudeCookiesRaw();
      const currentAuth = currentRaw.filter((c) => isAuthCookie(c.name)).map(serializeCookie);
      if (currentAuth.some((c) => c.name === "sessionKey" || c.name === "sessionKeyV3")) {
        postNative({
          type: "save_snapshot",
          account_uuid: before.accountUuid,
          email: before.email || null,
          orgs: before.orgs || [],
          cookies: currentAuth,
        });
      }
    }

    // 2. Remove existing auth & transient per-account cookies (leave Cloudflare/device cookies intact).
    const existing = await getAllClaudeCookiesRaw();
    for (const c of existing) {
      if (!shouldRemoveOnSwitch(c.name)) continue;
      await chrome.cookies.remove({
        url: cookieUrl(c),
        name: c.name,
        storeId: c.storeId,
      });
    }

    // 3. Write target account cookies (skip expired ephemeral sessionBinding cookies).
    const nowSec = Date.now() / 1000;
    let wroteLastActiveOrg = false;
    for (const rawC of targetCookies) {
      if (
        isEphemeralBindingCookie(rawC.name) &&
        typeof rawC.expirationDate === "number" &&
        rawC.expirationDate <= nowSec + 15
      ) {
        continue;
      }
      const c = { ...rawC };
      if (c.name === "lastActiveOrg" && targetOrgUuid) {
        c.value = targetOrgUuid;
        wroteLastActiveOrg = true;
      }
      await setCookieSafe(c);
    }
    if (targetOrgUuid && !wroteLastActiveOrg) {
      await setCookieSafe({
        name: "lastActiveOrg",
        value: targetOrgUuid,
        domain: ".claude.ai",
        hostOnly: false,
        path: "/",
        secure: true,
        httpOnly: false,
        sameSite: "lax",
        session: false,
        expirationDate: Math.floor(Date.now() / 1000) + 365 * 86400,
      });
    }

    // 4. Clear claude.ai web storage (does not remove cookies).
    await chrome.browsingData
      .remove({ origins: ["https://claude.ai"] }, { localStorage: true, indexedDB: true, cacheStorage: true })
      .catch(() => {});

    // 5. Verify via /api/bootstrap.
    const after = await probeBootstrap();
    const matched = !!after.signedIn && after.accountUuid === targetAccountUuid;

    // 6. Reload open claude.ai tabs (skip oauth/authorize & chrome/installed).
    const tabs = await chrome.tabs.query({ url: ["https://claude.ai/*"] });
    for (const tab of tabs) {
      if (!tab.id || !tab.url) continue;
      if (tab.url.includes("/oauth/authorize") || tab.url.includes("/chrome/installed")) continue;
      await chrome.tabs.reload(tab.id).catch(() => {});
    }

    // 7. Reset Claude in Chrome extension to drop old session tokens cleanly.
    let extReset = { ok: false, skipped: true, installed: false };
    if (matched && resetExt) {
      extReset = await resetClaudeExtension();
    }

    // 8. Arm short-lived auto-approve for claude.ai/oauth/authorize.
    let armedUntil = null;
    if (matched && armOAuth && extReset.installed) {
      const armed = await armOAuthAutoApprove({
        targetAccountUuid,
        targetOrgUuid,
        targetOrgName,
        autoCloseInstalledTab: autoClose,
      });
      armedUntil = Math.floor(armed.armedUntil / 1000);
    }

    const claudeExt = await getClaudeExtensionStatus();
    await savePopupStatus({
      signedIn: !!after.signedIn,
      email: after.email || "",
      claudeExt,
    });

    postNative({
      type: "switch_result",
      request_id: requestId,
      ok: matched,
      matched,
      account_uuid: after.accountUuid || null,
      email: after.email || null,
      orgs: after.orgs || [],
      claude_ext: claudeExt,
      ext_reset_ok: !!extReset.ok,
      armed_until: armedUntil,
      device_binding_lost: !!after.deviceBindingLost,
      cf_mitigated: after.cfMitigated || null,
      error: matched
        ? null
        : after.deviceBindingLost
          ? "Chrome 里的网页会话已失去设备绑定，请在 Chrome 重新登录该账号一次"
          : after.cfMitigated
            ? "被 Cloudflare 验证拦截，请先在 Chrome 打开 claude.ai 通过验证"
            : "该账号保存在 Chrome 的网页会话已过期，请在 Chrome 重新登录一次",
    });
  } catch (err) {
    postNative({
      type: "switch_result",
      request_id: requestId,
      ok: false,
      error: String(err),
    });
  } finally {
    suppressCookieEventsUntil = Date.now() + 2000;
  }
}

async function handleClearForLogin(msg) {
  const requestId = msg.request_id || "";
  suppressCookieEventsUntil = Date.now() + 6000;
  try {
    // Preserve current account in AuthBaton Vault before clearing local cookies.
    const before = await probeBootstrap();
    if (before.signedIn && before.accountUuid) {
      const currentRaw = await getAllClaudeCookiesRaw();
      const currentAuth = currentRaw.filter((c) => isAuthCookie(c.name)).map(serializeCookie);
      if (currentAuth.some((c) => c.name === "sessionKey" || c.name === "sessionKeyV3")) {
        postNative({
          type: "save_snapshot",
          account_uuid: before.accountUuid,
          email: before.email || null,
          orgs: before.orgs || [],
          cookies: currentAuth,
        });
      }
    }

    const existing = await getAllClaudeCookiesRaw();
    for (const c of existing) {
      if (!shouldRemoveOnSwitch(c.name)) continue;
      await chrome.cookies.remove({
        url: cookieUrl(c),
        name: c.name,
        storeId: c.storeId,
      });
    }
    await chrome.browsingData
      .remove({ origins: ["https://claude.ai"] }, { localStorage: true, indexedDB: true, cacheStorage: true })
      .catch(() => {});

    const tabs = await chrome.tabs.query({ url: ["https://claude.ai/*"] });
    if (tabs.length > 0) {
      for (const tab of tabs) {
        if (tab.id) await chrome.tabs.reload(tab.id).catch(() => {});
      }
      if (tabs[0].id) {
        await chrome.tabs.update(tabs[0].id, { active: true }).catch(() => {});
      }
    } else {
      await chrome.tabs.create({ url: "https://claude.ai/login" }).catch(() => {});
    }

    const claudeExt = await getClaudeExtensionStatus();
    await savePopupStatus({ signedIn: false, email: "", claudeExt });

    postNative({
      type: "clear_result",
      request_id: requestId,
      ok: true,
      claude_ext: claudeExt,
    });
  } catch (err) {
    postNative({
      type: "clear_result",
      request_id: requestId,
      ok: false,
      error: String(err),
    });
  } finally {
    suppressCookieEventsUntil = Date.now() + 1500;
  }
}

async function handleNativeMessage(msg) {
  if (!msg || typeof msg !== "object") return;
  switch (msg.type) {
    case "ping":
      postNative({ type: "pong", extension_version: EXTENSION_VERSION });
      break;
    case "capture_current":
      await captureAndReportState({ includeCookies: true, requestId: msg.request_id || null, reason: "capture" });
      break;
    case "switch_account":
      await handleSwitchAccount(msg);
      break;
    case "clear_for_login":
      await handleClearForLogin(msg);
      break;
    case "disarm_oauth":
      await disarmOAuth();
      break;
    default:
      break;
  }
}

function connectNative() {
  if (nativePort) return;
  try {
    const port = chrome.runtime.connectNative(NATIVE_HOST_NAME);
    nativePort = port;
    reconnectDelayMs = 2000;
    savePopupStatus({ connected: true, lastError: null });

    port.onMessage.addListener((msg) => {
      handleNativeMessage(msg).catch(() => {});
    });

    port.onDisconnect.addListener(() => {
      const err = chrome.runtime.lastError?.message || "disconnected";
      if (nativePort === port) {
        nativePort = null;
      }
      savePopupStatus({ connected: false, lastError: err });
      scheduleReconnect();
    });

    getClaudeExtensionStatus().then((claudeExt) => {
      postNative({
        type: "hello",
        extension_version: EXTENSION_VERSION,
        claude_ext: claudeExt,
      });
      captureAndReportState({ includeCookies: true, reason: "state" }).catch(() => {});
    });
  } catch (err) {
    disconnectNative(String(err));
  }
}

// Keep service worker connected to AuthBaton when Chrome is open.
chrome.alarms.create(KEEPALIVE_ALARM, { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name !== KEEPALIVE_ALARM) return;
  if (!nativePort) {
    connectNative();
  } else {
    postNative({ type: "pong", extension_version: EXTENSION_VERSION });
  }
});

chrome.runtime.onStartup.addListener(() => {
  connectNative();
});

chrome.runtime.onInstalled.addListener(() => {
  connectNative();
});

// Detect manual login on claude.ai and auto-report updated session to AuthBaton.
chrome.cookies.onChanged.addListener((changeInfo) => {
  const c = changeInfo?.cookie;
  if (!c || !c.domain || !c.domain.endsWith("claude.ai")) return;
  if (!isAuthCookie(c.name)) return;
  if (Date.now() < suppressCookieEventsUntil) return;
  if (cookieChangeTimer) clearTimeout(cookieChangeTimer);
  cookieChangeTimer = setTimeout(() => {
    cookieChangeTimer = null;
    if (!nativePort) connectNative();
    captureAndReportState({ includeCookies: true, reason: "state" }).catch(() => {});
  }, 1400);
});

// Watch OAuth completion redirect https://claude.ai/chrome/installed while armed.
chrome.tabs.onUpdated.addListener(async (tabId, changeInfo, tab) => {
  const url = changeInfo.url || tab?.url || "";
  if (!url.startsWith("https://claude.ai/chrome/installed")) return;
  const arm = await getOAuthArmState();
  if (!arm.armed) return;
  postNative({
    type: "oauth_event",
    event: "completed",
    account_uuid: arm.targetAccountUuid || null,
  });
  await disarmOAuth();
  if (arm.autoCloseInstalledTab) {
    await sleep(850);
    await chrome.tabs.remove(tabId).catch(() => {});
  }
});

// Messages from popup and oauth-auto-approve content script.
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  (async () => {
    switch (msg?.type) {
      case "GET_POPUP_STATE": {
        if (!nativePort) connectNative();
        const [bootstrap, claudeExt, arm] = await Promise.all([
          probeBootstrap(),
          getClaudeExtensionStatus(),
          getOAuthArmState(),
        ]);
        await savePopupStatus({
          signedIn: !!bootstrap.signedIn,
          email: bootstrap.email || "",
          claudeExt,
        });
        sendResponse({
          ok: true,
          extensionId: chrome.runtime.id,
          connected: !!nativePort,
          lastError: lastObservedStatus.lastError,
          bootstrap,
          claudeExt,
          arm,
        });
        break;
      }
      case "POPUP_CAPTURE_NOW": {
        if (!nativePort) connectNative();
        const res = await captureAndReportState({ includeCookies: true, reason: "capture" });
        sendResponse({ ok: res.ok, connected: !!nativePort });
        break;
      }
      case "POPUP_CLEAR_FOR_LOGIN": {
        if (!nativePort) connectNative();
        await handleClearForLogin({ request_id: "popup" });
        sendResponse({ ok: true });
        break;
      }
      case "GET_OAUTH_ARM_STATE": {
        sendResponse(await getOAuthArmState());
        break;
      }
      case "REPORT_OAUTH_EVENT": {
        const arm = await getOAuthArmState();
        if (msg.markClicked && arm.armed) {
          await chrome.storage.session.set({
            [ARM_KEY]: { ...arm, clickAttempted: true, clickedAt: Date.now() },
          });
        }
        postNative({
          type: "oauth_event",
          event: msg.event || "unknown",
          account_uuid: arm.targetAccountUuid || null,
          detail: typeof msg.detail === "string" ? msg.detail : null,
        });
        sendResponse({ ok: true });
        break;
      }
      default:
        sendResponse({ ok: false });
    }
  })().catch((err) => sendResponse({ ok: false, error: String(err) }));
  return true;
});

connectNative();
