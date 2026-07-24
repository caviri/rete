# Releasing the Java client to Maven Central

The Java client publishes to the **Sonatype Central Portal**
(<https://central.sonatype.com>) as two artifacts under group `io.github.caviri`:

- `io.github.caviri:rete-client`
- `io.github.caviri:rete-rdf4j`

Publishing is gated behind the `release` Maven profile so the normal build never
needs signing keys or the extra plugins. **A publish is irreversible** — a
released version can never be overwritten or deleted — so the profile is set to
`autoPublish=false`: it uploads and validates the bundle, then waits for you to
click **Publish** on the portal.

## Prerequisites (one-time)

1. **A verified namespace.** `io.github.caviri` must be verified on the Central
   Portal (add the TXT/GitHub verification the portal asks for). Until it is,
   uploads are rejected.
2. **A Central Portal user token.** Generate it under *Account → Generate User
   Token*. It is a `username:password` pair.
   - In this repo it lives in `.env` as `MAVEN_USERNAMEPASSWORD_BASE64`: the
     base64 encoding of that `username:password` string (quote it in `.env`).
3. **A GPG signing key.** Central requires every artifact to be PGP-signed.
   - Create one: `gpg --gen-key` (or `--full-generate-key`).
   - Publish the **public** key so Central can verify it:
     `gpg --keyserver keys.openpgp.org --send-keys <KEY_ID>`.
   - Note the key's passphrase.

## Release

From the repo root, with `.env` populated:

```sh
# 1. Export the credentials the way Maven's settings.xml expects.
set -a; . ./.env; set +a
CRED=$(printf '%s' "$MAVEN_USERNAMEPASSWORD_BASE64" | base64 -d)
export MAVEN_USERNAME="${CRED%%:*}"          # before the first colon
export MAVEN_PASSWORD="${CRED#*:}"           # after the first colon
export MAVEN_GPG_PASSPHRASE="<your gpg passphrase>"

# 2. Build, sign, and upload both modules (validated but NOT auto-released).
cd clients/java
mvn -B -s release-settings.xml -Prelease deploy

# 3. Go to https://central.sonatype.com → Deployments, review the validated
#    bundle, and click Publish. That step is the irreversible one.
```

The `-Prelease deploy` runs, per module: compile + test → source jar → javadoc
jar → GPG sign every artifact → upload the bundle to the portal.

### In Docker (mirrors the rest of the project)

The signing key has to be available inside the container:

```sh
gpg --export-secret-keys -a > /tmp/rete-signing.key   # export once
docker run --rm -v "$PWD:/work" -w /work/clients/java \
  -e MAVEN_USERNAME -e MAVEN_PASSWORD -e MAVEN_GPG_PASSPHRASE \
  -v /tmp/rete-signing.key:/key.asc:ro \
  maven:3.9-eclipse-temurin-21 \
  bash -lc 'gpg --batch --import /key.asc && \
            mvn -B -s release-settings.xml -Prelease deploy'
```

> Note: the wasm engine must be present in `rete-client`'s resources first (the
> Dockerfile stage 1 builds it; for a native release build run the cargo step
> from `.github/workflows/java-test.yml`).

## Verify the artifacts without publishing

To confirm the source and javadoc jars build (no key, no upload):

```sh
cd clients/java && mvn -B -Prelease -DskipTests package
# → target/*-sources.jar and *-javadoc.jar under each module
```

## Versioning

The artifacts track the engine version (`0.3.0`), kept in lockstep by
`ffi/Cargo.toml`, both module POMs, and the parent POM. Bump all of them together
for a new release. Central rejects re-publishing an existing version.
