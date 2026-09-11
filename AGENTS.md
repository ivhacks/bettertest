# AGENTS.md

This repo's Forgejo is on **van** (`van.local`), not GitHub. GitHub still exists as a second remote.

## Independent agent (gork)

You may optionally work as Linux user **gork** instead of the repo owner. That account is a separate Forgejo user (`gork`, `gork@iv.codes`) so commits and PRs are an independent agent, not iv.

On this machine (lappy):

- Linux user: `gork`
- Home: `/home/gork`
- Checkout: `/home/gork/bettertest`
- Switch: `sudo -u gork -H bash` or `sudo -u gork -H <command>`
- `iv` can sudo to `gork` with no password (`/etc/sudoers.d/gork-agent`)
- Git identity: `gork` / `gork@iv.codes`
- SSH key: `/home/gork/.ssh/id_ed25519`

When acting as gork, do not use iv's Forgejo credentials, iv's SSH keys, or an SSH login to `van@van.local`. If git or the API cannot authenticate as gork, ask the human. Do not work around it.

## Forgejo

- Web: http://van.local:3000
- API: http://van.local:3000/api/v1
- Canonical URL (`ROOT_URL`) is `http://van.local:3000/`. Using `localhost` or an IP makes the app show a mismatch banner.
- Registration is off. Sign-in is required to view the site in a browser.
- No mailer. Email is still a required unique field on every user; dummy addresses like `name@van.local` are fine. Login is username + password.
- van is an ASUS VivoBook running Fedora, set up as an always-on server (no suspend on lid close). Keep it plugged in; the battery is dead.

Swagger UI: http://van.local:3000/api/swagger

### Auth

Use the HTTP API. Header:

```http
Authorization: token <token>
```

Create a token in the web UI: **Settings → Applications → Generate New Token**. Scopes for PRs: `write:repository`. Do not SSH to van and run `forgejo admin` / sqlite.

Git push over HTTP uses the same token as the password (username `gork`). Git over SSH uses gork's key, which the human must add under **Settings → SSH / GPG Keys**.

SSH clone URL (system user is `forgejo`, not `git`):

```text
ssh://forgejo@van.local/OWNER/REPO.git
```

HTTP clone URL:

```text
http://van.local:3000/OWNER/REPO.git
```

### This repo's remotes

Default remote is Forgejo (`origin`). GitHub is extra.

```text
origin    http://van.local:3000/iv/bettertest.git
github    git@github.com:ivhacks/bettertest.git
```

`origin` is only the conventional fallback name. `git clone` names the remote `origin` unless you pass `-o`. After that it is just a label. Push/pull with no remote use (1) the branch upstream, (2) `remote.pushDefault`, (3) a remote actually named `origin`.

Default branch on Forgejo: `master`.

### Pull requests (as gork)

gork is not an admin and is not a collaborator on `iv/bettertest`. Fork, push a branch, open a PR via the API.

```bash
# fork
curl -sS -X POST \
  -H "Authorization: token $FORGEJO_TOKEN" \
  http://van.local:3000/api/v1/repos/iv/bettertest/forks

# remote for the fork
git remote add gork http://gork:${FORGEJO_TOKEN}@van.local:3000/gork/bettertest.git
git push -u gork HEAD

# open PR
curl -sS -X POST \
  -H "Authorization: token $FORGEJO_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title":"…","body":"…","head":"gork:BRANCH","base":"master"}' \
  http://van.local:3000/api/v1/repos/iv/bettertest/pulls
```

If `FORGEJO_TOKEN` is unset, ask the human for a gork token. Same if SSH push fails: give them `/home/gork/.ssh/id_ed25519.pub` and have them add it to the gork Forgejo account.

### Users (admin API only)

Email is required. Omit admin unless you mean it. These need an **admin** token, which gork does not have.

```bash
# list
curl -sS -H "Authorization: token $ADMIN_TOKEN" \
  http://van.local:3000/api/v1/admin/users

# create (no --admin equivalent: leave "admin": false)
curl -sS -X POST \
  -H "Authorization: token $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"username":"NAME","email":"NAME@van.local","password":"PASSWORD","must_change_password":true}' \
  http://van.local:3000/api/v1/admin/users

# delete
curl -sS -X DELETE \
  -H "Authorization: token $ADMIN_TOKEN" \
  http://van.local:3000/api/v1/admin/users/NAME
```

On the server the same operations exist as `forgejo admin user list|create|delete`, but this agent should not SSH to van to run them. `forgejo admin user create` is the CLI group; the flag that grants admin is `--admin`.
