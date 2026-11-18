# dockerignore-audit

A small Rust CLI and library that checks a Docker build context against its `.dockerignore` and lists the risky paths that would still be sent to the Docker daemon: `.env` files, private keys, the `.git` directory, a host `node_modules`, Terraform state and build caches. It also prints how many files and bytes the context contains. No dependencies.

## Why

`docker build .` sends the whole directory to the daemon, minus whatever `.dockerignore` excludes. With a `COPY . .` in the Dockerfile, everything that was sent lands in an image layer. Two surprises catch people:

- **Docker patterns are anchored at the context root.** Unlike `.gitignore`, a line `node_modules` only excludes the top-level folder. In a monorepo, `packages/web/node_modules` is still sent unless you write `**/node_modules`.
- **A secret deleted in a later layer is still in the image.** If `.env` was copied in one step and removed in the next, `docker history` and a layer export still reveal it. The only fix is never sending it.

dockerignore-audit applies the same matching rules Docker uses, including `**` and `!` re-includes where the last matching line wins, and shows what slips through.

## Install

```
cargo install dockerignore-audit
```

## Usage

```
dockerignore-audit            # current directory
dockerignore-audit ./services/api
```

Example output:

```
context: 1843 files, 212.4 MB sent; 12 files excluded
  send  .env.production                          environment file, may hold secrets
  send  .git                                     Git history, often large and may contain removed secrets
  send  packages/web/node_modules                host dependencies; install inside the image instead
3 risky path(s) would be sent; add them to .dockerignore
```

Exit code 1 when a risky path would be sent, so it can run in CI before `docker build`.

## A sensible starting .dockerignore

```
.git
**/node_modules
**/.env
**/.env.*
!**/.env.example
**/*.pem
**/__pycache__
**/.venv
target
coverage
.next/cache
```

## As a library

```rust
let rules = dockerignore_audit::Rules::parse("**/node_modules\n.env*\n");
assert!(rules.is_ignored("apps/site/node_modules/react/index.js"));
```

## Limits

Character classes like `[abc]` in patterns are treated literally, and symlinks are skipped.

A smaller context also means faster builds and smaller images; the guide on how to [reduce Docker image size](https://peon.sh/blogs/reduce-docker-image-size) covers the rest of the process. Built by a contributor to [Peon](https://peon.sh/), an open-source self-hosted deployment platform.

MIT licensed.
