FROM rust:slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked --bin morse

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates bash git curl openssh-client && rm -rf /var/lib/apt/lists/* && useradd --create-home --uid 10001 morse && install -d -o morse -g morse -m 700 /home/morse/.morse /home/morse/projects
COPY --from=build /src/target/release/morse /usr/local/bin/morse
USER morse
WORKDIR /home/morse
ENV MORSE_HOME=/home/morse/.morse
EXPOSE 7800
ENTRYPOINT ["morse"]
CMD ["serve", "--bind", "0.0.0.0:7800"]
