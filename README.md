<div align="center">
  <h1>iNilla — server</h1>
  <p><b>A vmangos 1.12.1 realm and a game-data host for the <a href="https://github.com/Martze96/iNilla">iNilla iOS client</a>, in one <code>docker compose up</code></b></p>
</div>

***This is basically vmangos with an added endpoint to serve WoW 1.12.1 Data so that users don't need to transfer it themselves and instead just point the [iNilla iOS client](https://github.com/Martze96/iNilla) to the endpoint.*** 

[**iNilla**](https://github.com/Martze96/iNilla) is a World of Warcraft 1.12.1 client for the iPhone. This repository is its companion home-network backend: everything the iOS app needs from a machine on your LAN.

| service | port | what the app uses it for |
|---|---|---|
| `realmd` | 3724 | logon: the server profile's host and port |
| `mangosd` | 8085 | the world: realmd hands the app `REALM_ADDRESS:8085` |
| `datahost` | 8099 | the one-time download of the game's `Data` folder (5.4 GB, resumable, SHA-256 verified), in English and optionally German, with the addons that go with it (pfQuest built in) |
| `db` | — | MariaDB for vmangos. Never published. |
| `db-init`, `extract` | — | one-shot jobs on every `up`: database setup/upgrades and map extraction. They exit when done. |

The server is [vmangos](https://github.com/vmangos/core) built from source at a pinned commit, with
its world database, the core's migrations, and `dbc`/`maps`/`vmaps` (and optionally `mmaps`)
extracted from **your own** client. Warden is off, because the iNilla client does not implement it.
Nothing from Blizzard is in this repository or in any image: your `Data` folder is mounted read-only.

## What you need

- **Docker**: Docker Desktop on Windows or macOS, or Docker Engine + Compose v2 on Linux. x86-64.
  The machine needs about 2 GB of free RAM while playing and about 10 GB of disk (mostly build
  cache; the realm itself is under 2 GB).
- **A World of Warcraft 1.12.1 client** (build 5875, English/enUS). Only its `Data` folder is used:
  `dbc.MPQ`, `patch.MPQ`, `terrain.MPQ` and the rest.
- **A fixed LAN address** for that machine (a DHCP reservation in your router), because the phone
  connects to it by address.

## Start

```sh
cp .env.example .env
```

In `.env`, set:

- **`WOW_DATA_DIR`** to your client's `Data` folder.
- **`REALM_ADDRESS`** to this machine's LAN IP.
- **The two database passwords** (`DB_ROOT_PASSWORD`, `DB_PASSWORD`). `openssl rand -hex 16`
  makes good ones.
- `INILLA_DATA_TOKEN` can stay empty on a home network: the phone then needs no token. Set one
  before the data host is reachable from the internet.

Then start the stack:

```sh
docker compose up -d
```

The first run takes about 10–30 minutes, and the command returns once everything has started:

1. It compiles vmangos (about 5–15 minutes, depending on your cores).
2. `db-init` imports the world database (under a minute).
3. `extract` pulls `dbc`, `maps` and `vmaps` out of your client (a few minutes).
4. `mangosd` loads the world.

Every later `up` takes seconds: each step checks for its own result first. Follow a step with
`docker compose logs -f extract`, for example.

Then create an account (1.12 has no sign-up, so accounts are made on the server):

```sh
docker compose exec mangosd vmangos-console account create alice s3cretpass
docker compose exec mangosd vmangos-console account set gmlevel alice 6   # optional: full GM
```

## Connect the iPhone

With the phone on the same Wi-Fi:

1. **Add the server.** In iNilla, go to Servers → Add:
   - host: your `REALM_ADDRESS`, port `3724`, server software **vmangos**
2. **Get the game data, once.** On the setup screen, open **Game data**. The URL is filled in
   from the server you added (`http://<REALM_ADDRESS>:8099/`); the token field stays empty unless
   you set one. Tap **Check server**, pick the language if the server offers more than one, then
   **Download**. It is 5.4 GB, and pfQuest installs right after it. Keep the app open and the
   screen on. An interrupted download resumes where it stopped, and every file is hash-checked
   before use.
   (You can also skip the download and copy the `Data` folder in with Finder or the Files app.)
3. **Allow Local Network access** when iOS asks. Without it, the app cannot reach anything on your LAN.
4. **Log in** with the account from above. The realm is listed as `REALM_NAME`.

Check the host from the phone's browser: `http://<REALM_ADDRESS>:8099/` should answer with one
line of text.

## Everyday use

```sh
docker compose ps                        # what is running (db-init and extract show "Exited (0)")
docker compose logs -f mangosd           # the world server's log
docker compose restart mangosd           # after editing config/mangosd.conf
docker compose stop                      # stop the realm (characters are saved)
docker compose up -d                     # start it again
docker compose exec mangosd vmangos-console server info
```

- **Server settings** (rates, the message of the day, the content patch and so on): edit
  [`config/mangosd.conf`](config/mangosd.conf) and [`config/realmd.conf`](config/realmd.conf), which
  hold `Key = Value` overrides applied over vmangos' defaults on every start.
- **The realm** (name, address, type): edit `.env`, then run `docker compose up -d` so `db-init`
  rewrites the realm row.

Accounts, GM commands, backups, updating vmangos, turning on mmaps and troubleshooting are covered
in **[docs/OPERATIONS.md](docs/OPERATIONS.md)**. How the pieces fit together, and why, is in
**[docs/DESIGN.md](docs/DESIGN.md)**.

## Only the data host, for a server you already run

Any 1.12.1 core with Warden off works with the app (vmangos, cMaNGOS). To give its players the
game data and pfQuest without this repository's realm, run the data host alone:

```sh
WOW_DATA_DIR="/path/to/WoW 1.12.1/Data" docker compose -f compose.datahost.yaml up -d
```

It needs nothing else from this stack. `WOW_DATA_DIR_DEDE`, `INILLA_DATA_TOKEN` and the
`addons/` folder work the same as in the full stack.

## Layout

| path | what |
|---|---|
| `compose.yaml` | the stack |
| `.env.example` | every setting, documented. Copy it to `.env`. |
| `config/` | overrides for `mangosd.conf` and `realmd.conf` |
| `docker/vmangos/` | the vmangos image: `Dockerfile` (compile at a pinned commit, world DB snapshot, runtime) and `bin/` (the roles: `inilla-db-init`, `inilla-extract`, `inilla-realmd`, `inilla-mangosd`, plus `vmangos-console`) |
| `datahost/` | `inilla-datahost`, the Rust service that serves the `Data` folder and the addons to the app |
| `addons/` | your own addons for the app, per language (see [addons/README.md](addons/README.md)) |
| `compose.datahost.yaml` | only the data host, beside a server you already run |
| `docs/` | operations and design notes |

## A word on copyright

The archives in your `Data` folder belong to Blizzard. Serving your own copy to your own phone is
between you and your copy of the game. Serving it to anyone else means distributing Blizzard's
files, so only share it with people who own the game, and set a token before the data host is
reachable from outside your home. iNilla is an
independent fan project, not affiliated with or endorsed by Blizzard Entertainment. vmangos is
GPLv2 and is built unmodified from its upstream source. Our own code is MIT OR Apache-2.0.
