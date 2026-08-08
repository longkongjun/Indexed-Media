# syntax=docker/dockerfile:1.7

ARG TARGETPLATFORM=linux/amd64

FROM --platform=${TARGETPLATFORM} node:24.18.0-bookworm-slim AS web-build
WORKDIR /src
RUN npm install --global pnpm@11.10.0
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/package.json
COPY packages/api-client-ts/package.json packages/api-client-ts/package.json
RUN pnpm install --frozen-lockfile --filter @mediaflow/web...
COPY apps/web apps/web
COPY packages/api-client-ts packages/api-client-ts
RUN pnpm --filter @mediaflow/web build

FROM --platform=${TARGETPLATFORM} rust:1.97.0-bookworm AS core-build
WORKDIR /src
COPY apps/core/Cargo.toml apps/core/Cargo.lock apps/core/
COPY apps/core/src apps/core/src
COPY apps/core/migrations apps/core/migrations
RUN cargo build --manifest-path apps/core/Cargo.toml --locked --release

FROM --platform=${TARGETPLATFORM} debian:bookworm-slim AS runtime
LABEL org.opencontainers.image.title="MediaFlow" \
      org.opencontainers.image.version="0.2.0-m2"

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends ca-certificates tzdata \
    && mkdir -p /app/web /run/mediaflow /config \
    && chown 10001:10001 /config \
    && chmod 0700 /config \
    && rm -rf /var/lib/apt/lists/* /var/cache/apt/* /usr/lib/apt /usr/bin/apt* /usr/bin/dpkg* /usr/sbin/dpkg* /var/lib/dpkg

COPY --from=core-build /src/apps/core/target/release/mediaflow-core /usr/local/bin/mediaflow-core
COPY --from=web-build --chown=10001:10001 /src/apps/web/dist/ /app/web/
COPY --chmod=755 infra/containers/healthcheck.sh /usr/local/bin/healthcheck.sh

ENV HOME=/tmp \
    MEDIAFLOW_MODE=production \
    MEDIAFLOW_LISTEN=0.0.0.0:3000 \
    MEDIAFLOW_CONFIG_DIR=/config \
    MEDIAFLOW_DEPLOYMENT_ROOTS_FILE=/run/mediaflow/deployment-roots.json \
    MEDIAFLOW_WEB_DIST=/app/web

EXPOSE 3000
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=6s --start-period=15s --retries=3 CMD ["/usr/local/bin/healthcheck.sh"]
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/mediaflow-core"]
CMD ["serve"]
