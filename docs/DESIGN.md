# Design

How the backend is put together, and why. For day-to-day commands, see [OPERATIONS.md](OPERATIONS.md).

## What the app needs from a server

The iNilla client is a native 1.12.1 client (benilla on iOS), so it needs exactly what the original
client needed, plus one thing a phone lacks:

1. **A 1.12.1 core without Warden.** The client speaks the original protocol (SRP6 logon on 3724,
   the world on 8085), reports itself as Windows x86 build 5875 with the matching integrity hash,
   and does not implement Warden. vmangos, which benilla develops against, accepts all of that with
   Warden off.
2. **A reachable world address.** realmd hands out the world server as `address:port` from its
   `realmlist` table. The phone dials that address, so it must be the LAN address of this machine
   and the published port, never `127.0.0.1` or a container name.
3. **The game data, once.** A desktop client reads `Data/` from disk. The phone gets it from
   `inilla-datahost` over HTTP. The alternative is copying the folder in with Finder or the Files app.

Everything else (TLS, WebSockets, a web front) is the browser clients' business, not this one's.

## Startup order

```
db (MariaDB, healthy) ──► db-init (one-shot) ──► realmd
                                          └────► mangosd ◄── extract (one-shot)
datahost (independent)
```

`db-init` and `extract` run on every `up` and exit 0. `realmd` and `mangosd` wait for them with
`service_completed_successfully`. Both one-shots check for their own results first, so after the
first run they take a second. A failure stops the start with the reason in the job's log.

## One image, four roles

`docker/vmangos/Dockerfile` builds vmangos once, and every vmangos service runs that image with a
different command:

| command | role |
|---|---|
| `inilla-db-init` | databases, user, release dump, migrations, realm row |
| `inilla-extract` | `dbc`/`maps` → `vmaps` → `mmaps` from the client |
| `inilla-realmd` | render `realmd.conf`, run realmd |
| `inilla-mangosd` | render `mangosd.conf`, run mangosd with a scriptable console |

The image tag is the vmangos commit (`inilla-vmangos:<sha>`). Changing `VMANGOS_COMMIT` in `.env`
therefore names an image that does not exist yet, and `docker compose up` builds it without being
told to.

The build follows upstream's own release workflow: Ubuntu 24.04, libmysqlclient, OpenSSL and zlib,
with `BUILD_FOR_HOST_CPU=0` so the image runs on any x86-64. Upstream's `contrib/docker-build` is
older than that workflow and no longer configures the tree.

## Configuration

The configs are regenerated from `/opt/vmangos/etc/*.conf.dist` on every start, in three layers:

1. **vmangos' defaults**, so new keys in a newer core arrive by themselves;
2. **what the stack owns**: database strings, paths, ports, `Console.Enable`, `GameType` from
   `REALM_TYPE`, Warden off, and vmap/mmap switches set by whether that data exists (vmangos refuses
   to start with mmaps on and none on disk, and runs without line of sight when vmaps are missing);
3. **`config/*.conf`**, the operator's `Key = Value` overrides.

The result lives in `/run/vmangos/` inside the container. It holds the database password, so it is
never written to a volume or the repository.

## Database

vmangos publishes its world database only as a release asset (`db_latest`, a dump named after the
commit it was taken at). The image carries that zip. `db-init` imports it into databases that are
still empty, then applies the core's `sql/migrations` that the database's `migrations` table does not
list yet. Every vmangos migration registers itself there, so this is exactly how vmangos expects an
upgrade to go, and an existing realm moves forward with each new `VMANGOS_COMMIT` without re-importing.

The pinned pair: core `53d24d5c7` (2026-10-07) and the `db-13b49dc` snapshot (2026-09-06). At
that commit the core and the dump hold the same migrations (9 logon, 1140 world, 8 characters,
7 logs). If a future snapshot is newer than the pinned core, `db-init` counts the unknown migrations
and warns.

MariaDB runs with `sql_mode=NO_ENGINE_SUBSTITUTION`. vmangos is MaNGOS-era code and expects the
permissive mode: zero dates, and silent truncation.

## Server data

`extract` writes into the `serverdata` volume, in the layout mangosd expects:

```
/vmangos/data/5875/dbc/    DBCs, namespaced by client build (mangosd reads DataDir/<build>/dbc/)
/vmangos/data/maps/        terrain and liquids (MapExtractor, high-resolution heights)
/vmangos/data/vmaps/       line of sight and indoor checks (VMapExtractor + VMapAssembler)
/vmangos/data/mmaps/       navmeshes (MoveMapGenerator via mmap_extract.py), optional
/vmangos/data/.inilla-*    one marker per finished step
```

On a 12-core machine, measured: `dbc` + `maps` take about 1 minute, `vmaps` about 2 minutes. mmaps
take hours, so they are opt-in and can be generated while the realm runs (see OPERATIONS.md).
mangosd mounts the volume read-only.

## The console

mangosd reads commands from stdin. When stdin closes, it shuts the world down cleanly, which in a
container looks like a crash loop with exit code 0. `inilla-mangosd` gives it a FIFO that the
process itself holds open read-write, so stdin never reaches EOF and a writer never blocks. mangosd's
output also goes through `tee` into `/run/vmangos/console.log`. `vmangos-console` truncates that
file, writes one command into the FIFO, and prints what arrives up to the next `mangos>` prompt.
That gives account creation and every other console command a plain `docker compose exec`, with no
TTY, no `docker attach`, and no SOAP or RA account to bootstrap first.

## The data host

`inilla-datahost` (Rust, `datahost/`) is the server half of the client's data contract:

| request | answer |
|---|---|
| `GET /` | a one-line hint, always open (also the health check) |
| `GET /locales.json` | `{"default":"enUS","locales":[{"locale","path":"deDE/","bytes","addons"}]}` |
| `GET [/<locale>]/manifest.json` | `{"format":1,"build":5875,"locale":"enUS","files":[{"name","size","sha256"}]}` |
| `GET\|HEAD [/<locale>]/files/<name>` | the archive. `Range: bytes=a-b` → `206`, so an interrupted download resumes |
| `GET [/<locale>]/addons.json` | `{"format":1,"locale","addons":[{"name","version","files":[{"path","size","sha256"}]}]}` |
| `GET\|HEAD [/<locale>]/addons/<path>` | one addon file |

Without a `/<locale>` prefix a request means the first `--data`, so an app that only knows
`/manifest.json` still works. A `--data` folder with no archives is skipped, which is how the
optional German one stays mounted empty. Addons come from `--addons` folders, for every language or
one: the image's pfQuest builds, then `addons/all`, `addons/<locale>`; a later folder's addon
replaces an earlier one of the same name.

With `INILLA_DATA_TOKEN` set, everything except `/` needs `Authorization: Bearer <token>` (or
`?token=`); empty, the host is open, which is the easy way on a home network. It serves only the
archives the client mounts (the eleven required ones plus `patch-?.MPQ` and `speech2.MPQ`), by
name from the manifest, never a path. Repack extras such as `base.MPQ` and `backup.MPQ` are left
out, because the client never opens them. The client refuses a manifest whose `format` or `build`
it does not know, or one that lacks a required archive, so a change to this shape must land in the
app too. `datahost/src/manifest.rs` and the client's `data` module have to agree.

Hashing 5.4 GB takes about 30 seconds from a Docker Desktop bind mount. The result is cached in the
`datahost` volume (`.inilla-manifest-<locale>.json`), keyed by every archive's name, size and mtime.

It was checked against the app's own download engine (the client's `data::download` over the same
`ureq` fetcher): the manifest is accepted, a wrong token gets 401, a download paused at 300 MB resumes
mid-file, and all 12 archives verify.

## Network and security

- Published: 3724, 8085, 8099, on `BIND_ADDR` (default all interfaces, because the phone is on the LAN).
- Not published: MariaDB. Its root and `mangos` passwords come from `.env`, and `db-init` resets the
  `mangos` password to match `.env` on every run.
- The data host is plain HTTP, open by default, which is fine on a home LAN. For the internet, put it
  behind HTTPS (the app speaks both) or reach it over a VPN such as Tailscale or WireGuard, and do
  not forward 8099 unprotected. Forwarding 3724 and 8085 to play from outside also needs
  `REALM_ADDRESS` to be the public address, or the app's world-host override on the phone.
- realmd's `localAddress` is set to the same address as `address`. Docker Desktop can make every
  client appear to come from its gateway, so the subnet test that picks between the two would not mean
  anything here.
