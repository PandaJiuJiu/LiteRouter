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
COPY backend/Cargo.toml backend/Cargo.lock* ./
COPY backend/src ./src
RUN cargo build --release
# re-copy and rebuild is cheap thanks to docker layer cache of the deps above

# ---------- stage 3: runtime ----------
FROM ${REGISTRY}/alpine:3.20
WORKDIR /app
RUN apk add --no-cache ca-certificates
COPY --from=backend /build/target/release/lite-one-api /app/lite-one-api
COPY --from=frontend /build/dist /app/dist

ENV PORT=3000
EXPOSE 3000
VOLUME ["/app/data"]

CMD ["/app/lite-one-api"]
