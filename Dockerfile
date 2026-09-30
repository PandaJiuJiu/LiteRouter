# Docker Hub may be unreachable in some networks; override with another mirror if needed:
#   docker-compose build --build-arg REGISTRY=docker.io/library
ARG REGISTRY=docker.m.daocloud.io/library

# ---------- stage 1: build frontend ----------
FROM ${REGISTRY}/node:20-alpine AS frontend
WORKDIR /build
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# ---------- stage 2: build backend ----------
FROM ${REGISTRY}/rust:1-alpine AS backend
WORKDIR /build
# Cargo registry mirror — crates.io direct fetch is unreliable in some CN networks.
# rsproxy is a widely-used Chinese mirror that supports both git and sparse protocols.
RUN mkdir -p /usr/local/cargo \
    && { \
       echo '[source.crates-io]'; \
       echo 'replace-with = "rsproxy"'; \
       echo '[source.rsproxy]'; \
       echo 'registry = "sparse+https://rsproxy.cn/index/"'; \
       echo '[registries.rsproxy]'; \
       echo 'index = "sparse+https://rsproxy.cn/index/"'; \
       } > /usr/local/cargo/config.toml
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
COPY backend/Cargo.toml backend/Cargo.lock* ./
COPY backend/src ./src
COPY backend/migrations ./migrations
RUN cargo build --release
# re-copy and rebuild is cheap thanks to docker layer cache of the deps above

# ---------- stage 3: runtime ----------
FROM ${REGISTRY}/alpine:3.20
WORKDIR /app
RUN apk add --no-cache ca-certificates
COPY --from=backend /build/target/release/literouter /app/literouter
COPY --from=frontend /build/dist /app/dist

ENV PORT=3000
EXPOSE 3000
VOLUME ["/app/data"]

CMD ["/app/literouter"]
