(() => {
  const CLAUDE_EXT_CLIENT_ID = "dae2cad8-15c5-43d2-9046-fcaecc135fa4";
  const APPROVE_PATTERNS = [
    /^authorize$/i,
    /^allow$/i,
    /^approve$/i,
    /^授权$/,
    /^允许$/,
    /^同意并授权$/,
  ];
  const REJECT_PATTERNS = [
    /cancel|deny|decline|reject|back|log\s*out|sign\s*out/i,
    /取消|拒绝|返回|退出/,
  ];

  function report(event, detail = null, markClicked = false) {
    chrome.runtime
      .sendMessage({ type: "REPORT_OAUTH_EVENT", event, detail, markClicked })
      .catch(() => {});
  }

  function isVisible(el) {
    if (!el || !(el instanceof HTMLElement)) return false;
    const style = window.getComputedStyle(el);
    if (style.display === "none" || style.visibility === "hidden" || style.opacity === "0") {
      return false;
    }
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  function getButtonLabel(el) {
    return (el.innerText || el.textContent || el.getAttribute("aria-label") || "")
      .replace(/\s+/g, " ")
      .trim();
  }

  function detectTurnstile() {
    return !!(
      document.querySelector('iframe[src*="challenges.cloudflare.com"]') ||
      document.querySelector('[name="cf-turnstile-response"]') ||
      document.querySelector("#cf-turnstile")
    );
  }

  function trySelectTargetOrg(arm) {
    if (!arm.targetOrgUuid && !arm.targetOrgName) return;

    const selects = Array.from(document.querySelectorAll("select")).filter(isVisible);
    for (const sel of selects) {
      const opt = Array.from(sel.options).find(
        (o) =>
          (arm.targetOrgUuid && o.value === arm.targetOrgUuid) ||
          (arm.targetOrgName && o.text.trim().toLowerCase() === arm.targetOrgName.trim().toLowerCase())
      );
      if (opt) {
        if (sel.value !== opt.value) {
          sel.value = opt.value;
          sel.dispatchEvent(new Event("change", { bubbles: true }));
        }
        return;
      }
    }

    if (arm.targetOrgName) {
      const targetLower = arm.targetOrgName.trim().toLowerCase();
      const radios = Array.from(document.querySelectorAll('[role="radio"], input[type="radio"], [role="option"]')).filter(
        isVisible
      );
      for (const r of radios) {
        const containerText = getButtonLabel(r.closest("label") || r).toLowerCase();
        if (containerText.includes(targetLower)) {
          r.click();
          return;
        }
      }
    }
  }

  function findApproveButton() {
    const candidates = Array.from(document.querySelectorAll('button, [role="button"], input[type="submit"]')).filter(
      isVisible
    );
    for (const el of candidates) {
      const label = getButtonLabel(el);
      if (!label) continue;
      if (REJECT_PATTERNS.some((re) => re.test(label))) continue;
      if (APPROVE_PATTERNS.some((re) => re.test(label))) {
        return { el, label };
      }
    }
    return null;
  }

  async function main() {
    const params = new URLSearchParams(location.search);
    const clientId = params.get("client_id") || "";
    const promptParam = params.get("prompt") || "";

    // Only act on Claude in Chrome interactive OAuth authorize requests.
    if (clientId !== CLAUDE_EXT_CLIENT_ID || promptParam === "none") {
      return;
    }

    const arm = await chrome.runtime.sendMessage({ type: "GET_OAUTH_ARM_STATE" }).catch(() => ({ armed: false }));
    if (!arm?.armed || arm.clickAttempted) return;

    let clicked = false;
    const deadline = Date.now() + 15000;

    const attempt = () => {
      if (clicked) return true;
      if (detectTurnstile()) {
        clicked = true;
        report("turnstile", "检测到 Cloudflare 验证，请手动完成授权");
        return true;
      }

      trySelectTargetOrg(arm);
      const found = findApproveButton();
      if (!found) return false;

      const isDisabled = !!found.el.disabled || found.el.getAttribute("aria-disabled") === "true";
      if (isDisabled) return false;

      clicked = true;
      found.el.click();
      report("clicked_authorize", found.label, true);

      // Only report stuck if the authorize button is still enabled (i.e. click was ignored).
      setTimeout(() => {
        if (!location.pathname.startsWith("/oauth/authorize")) return;
        const still = findApproveButton();
        if (still && !still.el.disabled && still.el.getAttribute("aria-disabled") !== "true") {
          report("click_ignored", "自动点击未生效，请手动点一次 Authorize");
        }
      }, 2500);
      return true;
    };

    if (attempt()) return;

    const obs = new MutationObserver(() => {
      if (attempt() || Date.now() > deadline) {
        obs.disconnect();
      }
    });
    obs.observe(document.documentElement, { childList: true, subtree: true, attributes: true });

    const timer = setInterval(() => {
      if (attempt() || Date.now() > deadline) {
        clearInterval(timer);
        obs.disconnect();
        if (!clicked) {
          report("timeout", "未检测到 Authorize 按钮，请手动完成授权");
        }
      }
    }, 300);
  }

  main();
})();
