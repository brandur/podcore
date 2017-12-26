# pod_core

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
