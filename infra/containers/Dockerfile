# syntax=docker/dockerfile:1.7

ARG TARGETPLATFORM=linux/amd64

FROM --platform=${TARGETPLATFORM} node:24.18.0-bookworm-slim@sha256:6f7b03f7c2c8e2e784dcf9295400527b9b1270fd37b7e9a7285cf83b6951452d AS web-build
WORKDIR /src
RUN npm install --global pnpm@11.10.0
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY patches patches
COPY apps/web/package.json apps/web/package.json
COPY packages/api-client-ts/package.json packages/api-client-ts/package.json
COPY packages/test-fixtures/package.json packages/test-fixtures/package.json
COPY packages/ui-vue/package.json packages/ui-vue/package.json
RUN pnpm install --frozen-lockfile --filter @mediaflow/web... --filter @mediaflow/test-fixtures
COPY apps/web apps/web
COPY packages/api-client-ts packages/api-client-ts
COPY packages/test-fixtures packages/test-fixtures
COPY packages/ui-vue packages/ui-vue
COPY packages/ui-contract/contract packages/ui-contract/contract
COPY contracts/examples contracts/examples
RUN pnpm --filter @mediaflow/web build

FROM --platform=${TARGETPLATFORM} rust:1.97.0-bookworm@sha256:8fa55b2f3ddf97471ab6a767bfa3f37e6bad0986ba823e75fea57e2a2a5c3073 AS core-build
WORKDIR /src
COPY apps/core/Cargo.toml apps/core/Cargo.lock apps/core/
COPY apps/core/src apps/core/src
COPY apps/core/migrations apps/core/migrations
RUN cargo build --manifest-path apps/core/Cargo.toml --locked --release

FROM --platform=${TARGETPLATFORM} debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8 AS runtime
ARG SOURCE_REVISION=unknown
ARG BUILD_CREATED=unrecorded
LABEL org.opencontainers.image.title="MediaFlow" \
      org.opencontainers.image.version="git-${SOURCE_REVISION}" \
      org.opencontainers.image.revision="${SOURCE_REVISION}" \
      org.opencontainers.image.created="${BUILD_CREATED}" \
      org.opencontainers.image.source="https://github.com/longkongjun/Indexed-Media"

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
