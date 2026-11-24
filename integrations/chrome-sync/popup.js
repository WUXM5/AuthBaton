const $ = (id) => document.getElementById(id);

async function refresh() {
  const res = await chrome.runtime.sendMessage({ type: "GET_POPUP_STATE" }).catch(() => null);
  if (!res?.ok) return;

  const badge = $("connBadge");
  if (res.connected) {
    badge.textContent = "已连接 AuthBaton";
    badge.className = "badge ok";
  } else {
    badge.textContent = "未连接 AuthBaton";
    badge.className = "badge warn";
  }

  const b = res.bootstrap || {};
  $("webVal").textContent = b.signedIn ? b.email || "已登录" : "未登录";

  const ext = res.claudeExt || {};
  $("extVal").textContent = ext.installed
    ? `${ext.enabled ? "已启用" : "已禁用"} v${ext.version}`
    : "未安装";

  const notice = $("armNotice");
  if (res.arm?.armed) {
    const remain = Math.max(0, Math.round((res.arm.armedUntil - Date.now()) / 1000));
    notice.style.display = "block";
    notice.textContent = `已就绪自动授权（剩 ${remain} 秒）：请点 Claude 插件里的 Log in。`;
  } else {
    notice.style.display = "none";
  }

  $("captureBtn").disabled = !res.connected || !b.signedIn;
}

$("captureBtn").addEventListener("click", async () => {
  $("captureBtn").disabled = true;
  try {
    const res = await chrome.runtime.sendMessage({ type: "POPUP_CAPTURE_NOW" });
    $("hint").textContent = res?.ok
      ? "已将当前 Chrome 的 claude.ai 登录保存到 AuthBaton。"
      : "当前未登录 claude.ai，或未能连上 AuthBaton。";
  } finally {
    await refresh();
  }
});

$("clearBtn").addEventListener("click", async () => {
  $("clearBtn").disabled = true;
  try {
    await chrome.runtime.sendMessage({ type: "POPUP_CLEAR_FOR_LOGIN" });
    $("hint").textContent = "已清空本地 Cookie（未注销服务端），请在打开的页面登录新账号。";
  } finally {
    $("clearBtn").disabled = false;
    await refresh();
  }
});

refresh();
