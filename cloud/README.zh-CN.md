# AuthBaton 个人云端

[English](README.md) | 简体中文

可选组件：部署在你自己 Cloudflare 账户里的 Worker + SQLite Durable Object，集中保存并续期 refresh token，让多台设备共用同一批账号。它不代理模型请求，也不记录请求体或凭据。macOS 客户端在「个人云端」页填写 Worker 地址并配对设备。

## Worker 配置

| 名称 | 类型 | 说明 |
|---|---|---|
| `VAULT_KEY` | Secret | 32 字节随机值的 base64，用于加密 Durable Object 里的凭据 |
| `ACCESS_TEAM` | 变量 | Cloudflare Access 团队名（`<team>.cloudflareaccess.com`） |
| `ACCESS_AUD` | 变量 | 保护该 Worker 的 Access 应用 AUD |
| `OWNER_EMAIL` | 变量 | 唯一允许管理的 Access 登录邮箱 |

缺任何一项，Worker 都返回 `deployment_not_configured`。管理页只接受通过 Access 校验的 `OWNER_EMAIL`。

## 命令

均在仓库根目录执行。构建和运行时测试使用你已经装好的工具目录（含 `wrangler`），脚本不会安装依赖，也不读取 Wrangler 的登录状态。

```bash
node --test cloud/test/*.test.ts                      # 合成 core/alarm 回归
node cloud/scripts/build.ts <工具目录>                 # 生成 dist/cloud/worker.js
node cloud/scripts/runtime-test.ts <工具目录>          # Miniflare + 全 mock 上游，验证实际 Worker 与持久重启
CLOUDFLARE_ACCOUNT_ID=<账户 ID> node cloud/scripts/deploy-personal.ts           # 只打印部署计划
CLOUDFLARE_ACCOUNT_ID=<账户 ID> node cloud/scripts/deploy-personal.ts --apply   # 上传 dist/cloud/worker.js
```

`--apply` 从 `dist/cloud-bootstrap/personal-api-token` 读取 API token，文件权限必须是 0600。也可以直接用 `wrangler deploy`（配置见 `wrangler.jsonc`）。
