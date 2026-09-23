# Addons for the app

The data host hands these to the iNilla app together with the game data. The app installs them
into its `AddOns` folder and updates them when they change here, like the game archives
(resumable, SHA-256 verified).

| folder | goes to |
|---|---|
| `all/` | every language |
| `enUS/` | players with English game data |
| `deDE/` | players with German game data |

Put each addon in as its own folder, the way 1.12 loads them: `all/MyAddon/MyAddon.toc` and the
rest of `MyAddon/` next to it. A folder without a `.toc` of its own name is ignored. Restart the
data host to pick changes up: `docker compose restart datahost`.

**pfQuest** (the quest helper) is built into the data host image, in English and German. An addon
here with the same name replaces the built-in one, for example a newer pfQuest.

Only share addons whose licence allows it. The app installs whatever the host offers.
