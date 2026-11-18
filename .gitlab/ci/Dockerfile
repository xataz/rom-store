FROM rust:1.100

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git jq make pkg-config libssl-dev \
	libsdl2-dev fonts-dejavu-core

ENTRYPOINT ["/bin/bash"]

