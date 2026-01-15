FROM rust:1.89-alpine AS builder

RUN apk add --no-cache build-base openssl-dev openssl-libs-static pkgconfig

WORKDIR /usr/src/app

COPY Cargo.toml Cargo.lock ./

RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release

RUN rm -rf src target/release/deps/hangar_back*

COPY src ./src

COPY migrations ./migrations

RUN cargo build --release

FROM alpine:latest AS runner

RUN apk add --no-cache libssl3 ca-certificates

# need to match host system's docker group
# 996 on my debian
# 965 on my arch
# verify with: getent group docker
RUN addgroup -g 996 docker

RUN addgroup -g 1000 appgroup && adduser -u 1000 -S appuser -G appgroup

RUN adduser appuser docker

RUN apk add --no-cache curl && \
    curl -sSfL https://raw.githubusercontent.com/anchore/grype/main/install.sh | sh -s -- -b /usr/local/bin && \
    grype version && \
    apk del curl

RUN grype version

RUN mkdir -p /home/appuser/.cache/grype && chown -R appuser:appgroup /home/appuser

WORKDIR /app

COPY --from=builder /usr/src/app/target/release/hangar_back .

COPY --from=builder /usr/src/app/migrations ./migrations

RUN chown appuser:appgroup hangar_back

USER appuser

EXPOSE 3000

CMD ["./hangar_back"]