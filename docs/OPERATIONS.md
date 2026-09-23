# Operations

Everything here runs from the repository root, where `compose.yaml` and `.env` are. PowerShell,
cmd and any Linux or macOS shell take the commands as written. Git Bash on Windows rewrites
arguments that look like paths (`/opt/…` becomes `C:/Program Files/Git/opt/…`), so there, prefix
those commands with `MSYS_NO_PATHCONV=1`.

## Accounts and GM levels

1.12 has no sign-up: accounts are made on the server, through mangosd's console. `vmangos-console`
sends one command and prints the answer:

```sh
docker compose exec mangosd vmangos-console account create alice s3cretpass
docker compose exec mangosd vmangos-console account set gmlevel alice 6
docker compose exec mangosd vmangos-console account set password alice newpass newpass
docker compose exec mangosd vmangos-console account onlinelist
docker compose exec mangosd vmangos-console account delete alice
```

vmangos stores names and passwords upper-cased, and the app sends them that way too, so
capitalisation does not matter when logging in.

GM levels: `0` player (the default), `1` moderator, `2` ticket master, `3` game master,
`4` basic admin, `5` developer, `6` administrator. The iNilla client's development probes expect
their test account at `6`. GM commands are typed in chat with a leading dot (`.gm on`,
`.tele Stormwind`, `.help`). On the console, leave the dot off:

```sh
docker compose exec mangosd vmangos-console server info
docker compose exec mangosd vmangos-console announce Server restarts in 10 minutes
docker compose exec mangosd vmangos-console help
```

The world has to be loaded first. When `docker compose ps` shows mangosd as `healthy`, it is.

## Settings

| what | where | apply with |
|---|---|---|
| realm name, address, type (pve/pvp/rp/rppvp), ports | `.env` | `docker compose up -d` |
| rates, MOTD, content patch, player limits, … | `config/mangosd.conf` | `docker compose restart mangosd` |
| logon throttling, … | `config/realmd.conf` | `docker compose restart realmd` |

The override files hold `Key = Value` lines, applied over vmangos' defaults on every start. The
stack manages the database strings, paths, ports, the console, Warden and the vmap/mmap switches
itself. An override file wins even over those, so leave them out unless you mean it. To see every
key with its documentation:

```sh
docker compose exec mangosd cat /opt/vmangos/etc/mangosd.conf.dist
```

To see what mangosd actually runs with: `docker compose exec mangosd cat /run/vmangos/mangosd.conf`.

## Movement maps (mmaps)

Without mmaps, creatures chase you in straight lines, through trees and up cliffs. Generating
mmaps takes hours on the first run, and the realm keeps running while they are made:

```sh
docker compose run --rm -e EXTRACT_MMAPS=1 extract    # hours; uses every core
docker compose restart mangosd                          # mmaps turn on because they now exist
```

Also set `EXTRACT_MMAPS=1` in `.env`, so a fresh machine builds them too.

## Backups

The world database can be rebuilt at any time. Accounts (`realmd`) and characters (`characters`)
cannot, so back those up:

```sh
docker compose exec db sh -c 'mariadb-dump -uroot -p"$MARIADB_ROOT_PASSWORD" --single-transaction --databases realmd characters > /tmp/inilla-backup.sql'
docker compose cp db:/tmp/inilla-backup.sql ./inilla-backup.sql
```

To restore:

```sh
docker compose stop mangosd realmd
docker compose cp ./inilla-backup.sql db:/tmp/inilla-backup.sql
docker compose exec db sh -c 'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" < /tmp/inilla-backup.sql'
docker compose up -d
```

`docker compose up -d` at the end also re-applies the realm row from `.env`, which the backup
overwrote.

## Updating vmangos

1. Back up (above).
2. Set `VMANGOS_COMMIT` in `.env` to a newer commit of
   [vmangos/core `development`](https://github.com/vmangos/core/commits/development).
3. Run `docker compose up -d`. The image tag contains the commit, so compose builds the new image by
   itself. Then `db-init` applies the new migrations and mangosd restarts on the new core.

A new image also means a new world-database snapshot, but only a *fresh* database imports it. An
existing one is brought forward by the core's migrations, which is how vmangos is meant to be
upgraded. If `db-init` warns that the database is newer than the core, the snapshot outran your
pinned commit: move `VMANGOS_COMMIT` forward.

On rare occasions vmangos changes its map format, and mangosd then refuses the old map files in its
log. To re-extract:

```sh
docker compose down
docker volume rm inilla_serverdata
docker compose up -d
```

## Logs

```sh
docker compose logs -f mangosd          # console output
docker compose logs --since 1h realmd
docker compose exec mangosd ls /vmangos/logs   # Server.log, DBErrors.log, Chat.log, weekly honor, …
```

## Moving to another machine, or a new address

- **A new LAN address:** change `REALM_ADDRESS` in `.env`, run `docker compose up -d`, and update
  the server profile in the app.
- **Another machine:** back up, clone this repository there, copy `.env` over (fixing
  `WOW_DATA_DIR` and `REALM_ADDRESS`), run `docker compose up -d`, and restore the backup.

## Troubleshooting

**The app cannot reach the server.** Open `http://<REALM_ADDRESS>:8099/` in Safari on the phone.

- If Safari gets no answer, the machine is not reachable. Check that the phone is on the same
  network (not a guest Wi-Fi), that `BIND_ADDR` is `0.0.0.0`, and the firewall:
  - **Windows** (Docker Desktop): the firewall rule is for the program, not the port. The
    *Docker Desktop Backend* rule must allow inbound traffic on the profile your network uses.
    Check with `Get-NetFirewallRule -DisplayName "Docker Desktop Backend"`.
  - **Linux** (ufw): `sudo ufw allow 3724,8085,8099/tcp`.
- If Safari works but the app does not, iOS has refused Local Network access. Allow it under
  Settings → Privacy & Security → Local Network → iNilla.

**Logon works, but entering the world fails or hangs.** realmd hands out `REALM_ADDRESS:MANGOSD_PORT`
as the world server, so that address has to be reachable from the phone. Check what realmd hands out:

```sh
docker compose exec db sh -c 'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" -e "SELECT name, address, port, realmflags FROM realmd.realmlist"'
```

The app's server profile can also override the world-server host.

**The realm shows as offline.** mangosd is down or still loading: `docker compose ps` and
`docker compose logs mangosd`.

**`docker compose up` fails on `extract` or `db-init`.** Their logs say why:
`docker compose logs extract db-init`. Both are safe to re-run: `docker compose up -d`.

**`vmangos-console` says there was no prompt after 15 s.** The world is still loading.

**Start over from nothing.** `docker compose down -v` deletes the database (accounts and
characters) and the extracted server data. The next `up` rebuilds both.
