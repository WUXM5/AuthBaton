# AuthBaton personal cloud

English | [简体中文](README.zh-CN.md)

An optional component: a Worker with a SQLite Durable Object in your own Cloudflare account. It stores and renews refresh tokens in one place, so several devices can share the same accounts. It does not proxy model requests and does not record request bodies or credentials. In the macOS app, open the 个人云端 (Personal cloud) page, enter the Worker address and pair the device.

## Worker configuration

| Name | Type | Description |
|---|---|---|
| `VAULT_KEY` | Secret | Base64 of 32 random bytes, used to encrypt the credentials in the Durable Object |
| `ACCESS_TEAM` | Variable | Cloudflare Access team name (`<team>.cloudflareaccess.com`) |
| `ACCESS_AUD` | Variable | AUD of the Access application that protects this Worker |
| `OWNER_EMAIL` | Variable | The only Access sign-in email allowed to manage the Worker |

If any of them is missing, the Worker returns `deployment_not_configured`. The management page accepts only `OWNER_EMAIL`, after Access has verified it.

## Commands

Run all commands from the repository root. The build and the runtime test use a tools directory you have already set up (including `wrangler`). The scripts do not install dependencies and do not read Wrangler's sign-in state.

```bash
node --test cloud/test/*.test.ts                      # synthetic core/alarm regression tests
node cloud/scripts/build.ts <tools dir>               # writes dist/cloud/worker.js
node cloud/scripts/runtime-test.ts <tools dir>        # Miniflare with every upstream mocked: checks the real Worker and a restart with persisted state
CLOUDFLARE_ACCOUNT_ID=<account ID> node cloud/scripts/deploy-personal.ts           # prints the deployment plan only
CLOUDFLARE_ACCOUNT_ID=<account ID> node cloud/scripts/deploy-personal.ts --apply   # uploads dist/cloud/worker.js
```

`--apply` reads the API token from `dist/cloud-bootstrap/personal-api-token`, which must have mode 0600. You can also run `wrangler deploy` directly; the configuration is in `wrangler.jsonc`.
