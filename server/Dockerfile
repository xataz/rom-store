FROM docker.io/library/rust:1.90.0-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM docker.io/library/debian:bookworm-slim
RUN groupadd --system --gid 10001 romstore \
    && useradd --system --uid 10001 --gid romstore romstore \
    && install -d -o romstore -g romstore /data
COPY --from=build /src/target/release/romstore-server /usr/local/bin/romstore-server
USER 10001:10001
EXPOSE 8000
ENTRYPOINT ["romstore-server"]
CMD ["serve", "--library", "/library", "--database", "/data/catalog.sqlite3", "--system-icon", "/icons", "--host", "0.0.0.0"]
