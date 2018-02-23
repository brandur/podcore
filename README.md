# podcore [![Build Status](https://travis-ci.org/brandur/podcore.svg?branch=master)](https://travis-ci.org/brandur/podcore)

```
brew install direnv
cargo install diesel_cli --no-default-features --features postgres
```

```
cp .envrc.sample .envrc
direnv allow

# $DATABASE_URL
createdb podcore
diesel migration run

# $TEST_DATABASE_URL
createdb podcore-test
DATABASE_URL=$TEST_DATABASE_URL diesel migration run
```

```
$ cargo build && target/debug/podcore api
GraphQL server started on 0.0.0.0:8080
```

Sample query (WIP):

```
$ curl -X POST http://localhost:8080/graphql -d 'query xxx { yyy }
```

GraphiQL is available at [localhost:8080](http://localhost:8080).

Schema changes:

```
diesel print-schema > src/schema.rs
```

Rustfmt:

```
rustup component add --toolchain=nightly rustfmt-preview
cargo +nightly fmt
```

Tests:

```
cargo test

# run a single test (matches on name)
cargo test test_minimal_feed

# show stdout (note that `cargo test -- --nocapture` doesn't work because it
# only affects print! and println! macros)
RUST_TEST_NOCAPTURE=1 cargo test
```

## Kubernetes

Build an Alpine-based binary target for MUSL, push to GCP container registry,
then run Kubernetes deployment:

```
docker build -t gcr.io/${PROJECT_ID}/podcore:1.25 .
gcloud docker -- push gcr.io/${PROJECT_ID}/podcore:1.25
kubectl apply -f kubernetes/
kubectl apply -f kubernetes/podcore-crawl.yaml
kubectl logs -l name=podcore-crawl -c podcore
```

Migrations can be run with:

```
kubectl get pods
kubectl exec podcore-crawl-1413166641-rqb9p -c podcore -- /podcore migration
```

<!--
# vim: set tw=79:
-->
