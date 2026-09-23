# Shared helpers for the iNilla vmangos roles. Sourced, never run.

log() { printf '[%s] %s\n' "${INILLA_ROLE:-inilla}" "$*" >&2; }
die() {
    log "ERROR: $*"
    exit 1
}

: "${DB_HOST:=db}"
: "${DB_PORT:=3306}"
: "${DB_USER:=mangos}"
: "${DB_PASSWORD:=}"
# vmangos' own config defaults, so an untouched .conf.dist would already point at them.
DB_REALMD=realmd
DB_WORLD=mangos
DB_CHARACTERS=characters
DB_LOGS=logs

# The password ends up inside a `host;port;user;password;database` string in the vmangos configs
# and inside SQL in db-init, so it is held to characters neither of those can misread.
check_db_password() {
    [ -n "$DB_PASSWORD" ] || die "DB_PASSWORD is empty — set it in .env"
    case "$DB_PASSWORD" in
        *[!A-Za-z0-9._-]*) die "DB_PASSWORD may only use letters, digits, '.', '_' and '-'" ;;
    esac
}

# The quoted connection string vmangos expects for database $1.
db_info() { printf '"%s;%s;%s;%s;%s"' "$DB_HOST" "$DB_PORT" "$DB_USER" "$DB_PASSWORD" "$1"; }

# Block until the database answers as $1 (password $2), for up to five minutes.
wait_for_db() {
    local user="$1" pass="$2" i
    for i in $(seq 1 150); do
        if mariadb-admin --host="$DB_HOST" --port="$DB_PORT" --user="$user" --password="$pass" \
            --connect-timeout=2 ping >/dev/null 2>&1; then
            return 0
        fi
        [ "$i" = 1 ] && log "waiting for the database at $DB_HOST:$DB_PORT"
        sleep 2
    done
    die "the database at $DB_HOST:$DB_PORT did not answer as '$user'"
}

# conf_set FILE KEY VALUE — rewrite KEY's line (case-insensitive, the first uncommented one), or
# append it. VALUE is written as given, so string values carry their own quotes.
conf_set() {
    local file="$1" key="$2" value="$3"
    awk -v k="$key" -v v="$value" '
        BEGIN { done = 0; lk = tolower(k) }
        {
            line = $0
            sub(/\r$/, "", line)
            if (!done && line !~ /^[ \t]*[#;]/ && index(line, "=") > 0) {
                name = substr(line, 1, index(line, "=") - 1)
                gsub(/^[ \t]+|[ \t]+$/, "", name)
                if (tolower(name) == lk) { print k " = " v; done = 1; next }
            }
            print line
        }
        END { if (!done) print k " = " v }
    ' "$file" >"$file.tmp" && mv "$file.tmp" "$file"
}

# conf_apply_overrides FILE OVERRIDES — every `Key = Value` line of OVERRIDES onto FILE.
conf_apply_overrides() {
    local file="$1" overrides="$2" line key value n=0
    [ -f "$overrides" ] || return 0
    while IFS= read -r line || [ -n "$line" ]; do
        line="${line%$'\r'}"
        case "$line" in '' | '#'* | ';'*) continue ;; esac
        if [ "${line#*=}" = "$line" ]; then
            log "ignoring a line without '=' in $(basename "$overrides"): $line"
            continue
        fi
        key="${line%%=*}"
        value="${line#*=}"
        key="$(printf '%s' "$key" | sed -E 's/^[[:space:]]+|[[:space:]]+$//g')"
        value="$(printf '%s' "$value" | sed -E 's/^[[:space:]]+|[[:space:]]+$//g')"
        [ -n "$key" ] || continue
        conf_set "$file" "$key" "$value"
        n=$((n + 1))
    done <"$overrides"
    [ "$n" = 0 ] || log "applied $n override(s) from $(basename "$overrides")"
}

# The realm type as vmangos numbers it (realmlist.icon and mangosd's GameType share the values).
realm_type_id() {
    case "$(printf '%s' "${REALM_TYPE:-pve}" | tr '[:upper:]' '[:lower:]')" in
        pve | normal) echo 0 ;;
        pvp) echo 1 ;;
        rp) echo 6 ;;
        rppvp | rp-pvp) echo 8 ;;
        *) die "REALM_TYPE must be pve, pvp, rp or rppvp (got '${REALM_TYPE}')" ;;
    esac
}
