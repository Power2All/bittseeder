FROM rust:alpine

RUN apk update --no-interactive
RUN apk add git musl-dev curl pkgconfig openssl-dev openssl-libs-static --no-interactive
RUN git clone https://github.com/Power2All/bittseeder.git /app/bittseeder
RUN cd /app/bittseeder && git checkout tags/v0.1.1
WORKDIR /app/bittseeder
RUN cd /app/bittseeder
RUN cargo build --release && rm -Rf target/release/.fingerprint target/release/build target/release/deps target/release/examples target/release/incremental
COPY init.sh /app/bittseeder/target/release/init.sh
COPY healthcheck.py /app/bittseeder/healthcheck
RUN chmod +x /app/bittseeder/target/release/init.sh
RUN chmod +x /app/bittseeder/healthcheck
EXPOSE 6881/tcp
EXPOSE 6881/udp
EXPOSE 8090/tcp
EXPOSE 80/tcp
CMD /app/bittseeder/target/release/init.sh