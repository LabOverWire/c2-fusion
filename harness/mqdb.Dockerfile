# Repackage the upstream mqdb musl binary (its image is FROM scratch, no writable
# fs) onto alpine, and bake in dev credentials + ACL so no read-only mounts are
# needed at runtime. POC-only credentials.
FROM mqdb:poc AS src
FROM alpine:3.20
COPY --from=src /mqdb /usr/local/bin/mqdb
RUN mkdir -p /data /auth \
 && /usr/local/bin/mqdb passwd c2 -b c2pass -n > /auth/passwd.txt \
 && printf 'user c2 topic $DB/# permission readwrite\nuser c2 topic p2p/# permission readwrite\nuser c2 topic +/responses permission readwrite\n' > /auth/acl.txt
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/mqdb"]
