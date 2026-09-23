# syntax=docker/dockerfile:1
#
# vmangos (World of Warcraft 1.12.1, build 5875) built from source at a pinned commit.
#
# One image plays every vmangos role in compose.yaml: realmd, mangosd, the one-shot database
# initialiser (db-init) and the one-shot map extractor (extract). The role is the command.
#
# The build follows upstream's own release pipeline (.github/workflows/development-release.yaml):
# Ubuntu 24.04, libmysqlclient, OpenSSL, zlib, no ACE or TBB any more. Upstream's
# contrib/docker-build predates that and no longer configures the tree.
#
# Build context is the repository root (compose.yaml does this):
#   docker build -f docker/vmangos/Dockerfile --build-arg VMANGOS_COMMIT=<sha> -t inilla-vmangos .

ARG UBUNTU_VERSION=24.04

# ---- compile ------------------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION} AS build
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        cmake \
        git \
        libmysqlclient-dev \
        libssl-dev \
        zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

ARG VMANGOS_REPO=https://github.com/vmangos/core.git
ARG VMANGOS_COMMIT
# A full commit, fetched on its own: a branch name would make the build depend on the day it ran.
RUN test -n "${VMANGOS_COMMIT}" || { echo "VMANGOS_COMMIT build arg is required" >&2; exit 1; } \
    && git init -q /src \
    && git -C /src remote add origin "${VMANGOS_REPO}" \
    && git -C /src fetch -q --depth 1 origin "${VMANGOS_COMMIT}" \
    && git -C /src checkout -q FETCH_HEAD

# SUPPORTED_CLIENT_BUILD keeps its default (1.12.1). BUILD_FOR_HOST_CPU=0 so the image runs on any
# x86-64, not just the machine that built it. The extractors turn the client's MPQs into the
# dbc/maps/vmaps/mmaps mangosd needs.
RUN cmake -S /src -B /build \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX=/opt/vmangos \
        -DBUILD_EXTRACTORS=1 \
        -DBUILD_FOR_HOST_CPU=0 \
        -DDEBUG_SYMBOLS=0 \
    && cmake --build /build -j"$(nproc)" \
    && cmake --install /build \
    && mkdir -p /opt/vmangos/sql/migrations \
    && cp /src/sql/migrations/*_world.sql /src/sql/migrations/*_logon.sql \
          /src/sql/migrations/*_characters.sql /src/sql/migrations/*_logs.sql \
          /opt/vmangos/sql/migrations/ \
    && git -C /src rev-parse HEAD > /opt/vmangos/sql/core-commit

# ---- world database snapshot --------------------------------------------------------------------
# vmangos publishes the world database only as a release asset (tag db_latest), named after the
# commit it was dumped at. Empty VMANGOS_DB_URL means "whatever db_latest holds right now"; db-init
# then brings it up to this image's core with the core's own migrations.
FROM ubuntu:${UBUNTU_VERSION} AS db
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
ARG VMANGOS_DB_URL=
RUN url="${VMANGOS_DB_URL}" \
    && if [ -z "$url" ]; then \
         url="$(curl -fsSL https://api.github.com/repos/vmangos/core/releases/tags/db_latest \
                | grep -oE 'https://[^"]*/db-[0-9a-f]+\.zip' | head -1)"; \
       fi \
    && test -n "$url" || { echo "could not find the db_latest release asset" >&2; exit 1; } \
    && echo "world database: $url" \
    && curl -fsSL -o /db.zip "$url" \
    && echo "$url" > /db.url

# ---- runtime ------------------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION}
ARG DEBIAN_FRONTEND=noninteractive
# mariadb-client and unzip are for db-init, python3 for the mmap generator's driver script.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libmysqlclient21 \
        libssl3t64 \
        mariadb-client \
        python3 \
        tzdata \
        unzip \
        zlib1g \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /opt/vmangos /opt/vmangos
COPY --from=db /db.zip /db.url /opt/vmangos/sql/
COPY docker/vmangos/bin/ /usr/local/bin/

# A checkout with CRLF endings would hand the shell `bash\r`; strip them rather than trust the
# checkout. Then fail the build now, not at first start, if a library the binaries need is missing.
RUN sed -i 's/\r$//' /usr/local/bin/inilla-* /usr/local/bin/vmangos-console \
    && chmod 0755 /usr/local/bin/inilla-* /usr/local/bin/vmangos-console \
    && ! ldd /opt/vmangos/bin/mangosd /opt/vmangos/bin/realmd /opt/vmangos/bin/Extractors/* 2>/dev/null \
         | grep 'not found' \
    && groupadd --system vmangos \
    && useradd --system --gid vmangos --home-dir /vmangos --shell /usr/sbin/nologin vmangos \
    && mkdir -p /vmangos/data /vmangos/logs /run/vmangos \
    && chown -R vmangos:vmangos /vmangos /run/vmangos

ARG VMANGOS_COMMIT
LABEL org.opencontainers.image.title="iNilla vmangos" \
      org.opencontainers.image.description="vmangos 1.12.1 (build 5875) for the iNilla iOS client" \
      org.opencontainers.image.source="https://github.com/vmangos/core" \
      org.opencontainers.image.revision="${VMANGOS_COMMIT}"

ENV PATH=/opt/vmangos/bin:/opt/vmangos/bin/Extractors:${PATH}
USER vmangos
WORKDIR /vmangos
# No default role: compose.yaml names one per service.
CMD ["sh", "-c", "echo 'give a role: inilla-realmd, inilla-mangosd, inilla-db-init or inilla-extract' >&2; exit 2"]
