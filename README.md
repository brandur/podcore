# pod_core [![Build Status](https://travis-ci.org/brandur/pod_core.svg?branch=master)](https://travis-ci.org/brandur/pod_core)

```
cargo install diesel_cli --no-default-features --features postgres
createdb pod_core
echo "export DATABASE_URL=postgres://localhost/pod_core" > .envrc
direnv allow
```

```
$ cargo build && target/debug/pod_core
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

Rustfmt (run on `nightly` because rustfmt can't seem to detach itself from
`nightly`)::

```
rustup install nightly
cargo install rustfmt-nightly
rustup run nightly cargo fmt
```

<!--
# vim: set tw=79:
-->
