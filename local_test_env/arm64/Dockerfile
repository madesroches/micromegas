FROM rust:latest 

RUN apt update; apt upgrade -y
RUN apt install -y g++-aarch64-linux-gnu libc6-dev-arm64-cross
RUN rustup target add aarch64-unknown-linux-gnu
RUN rustup toolchain install stable-aarch64-unknown-linux-gnu

RUN wget https://www.openssl.org/source/openssl-3.3.0.tar.gz
RUN tar zxf openssl-3.3.0.tar.gz
RUN cd openssl-3.3.0 && \
	./Configure linux-aarch64 --cross-compile-prefix=/usr/bin/aarch64-linux-gnu- --prefix=/opt/openssl-3.3.0 --openssldir=/opt/openssl-3.3.0 -static && \
	make install
ENV	OPENSSL_DIR=/opt/openssl-3.3.0
