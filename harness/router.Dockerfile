FROM alpine:3.20
RUN apk add --no-cache iproute2 tcpdump
ENTRYPOINT ["tail", "-f", "/dev/null"]
