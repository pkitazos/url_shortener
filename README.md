# URL shortener

## Sequence Diagram
```mmd
sequenceDiagram
    participant C as Client
    participant S as Server
    participant D as Database

    alt shorten
        C->>S: shorten(long_url: str)
        S->>D: store(mapping: URL)
        D->>S: ok()
        S->>C: success(short_code: str)
    else redirect
        C->>S: redirect(short_code: str)
        S->>D: lookup(short_code: str)
        alt not found
            D->>S: not_found()
            S->>C: not_found()
        else found
            D->>S: ok(mapping: URL)
            S->>C: found(long_url: str)
        end
    end
```

## Protocol
```
C -> S : {
    shorten(long_url: str) . S -> D : store(mapping: URL) . D -> S : ok() . S -> C : success(short_code: str) . end,
    redirect(short_code: str) . S -> D : lookup(short_code: str) . D -> S : {
        not_found() . S -> C : not_found() . end,
        ok(mapping: URL) . S -> C : found(long_url: str) . end
    }
}
```
