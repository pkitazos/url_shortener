use std::{
    collections::HashMap,
    error::Error,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use sqlx::{FromRow, Pool, Sqlite, SqlitePool};

#[derive(Debug, Clone)]
struct AppCtx {
    pool: Pool<Sqlite>,
    short_to_long_cache: Arc<Mutex<HashMap<String, String>>>,
    long_to_short_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl AppCtx {
    fn new(pool: Pool<Sqlite>) -> AppCtx {
        AppCtx {
            short_to_long_cache: Arc::new(Mutex::new(HashMap::new())),
            long_to_short_cache: Arc::new(Mutex::new(HashMap::new())),
            pool,
        }
    }
}

#[derive(FromRow)]
struct URL {
    long_url: String,
    short_code: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let pool = SqlitePool::connect("sqlite:urlshortener.db").await?; // expects the file to already exist

    sqlx::migrate!("./migrations").run(&pool).await?;

    println!("created db");

    let app = Router::new()
        .route("/", get(root))
        .route("/shorten", post(shorten)) // passing the long url as a query param
        .route("/redirect/{short_code}", get(redirect))
        .with_state(AppCtx::new(pool));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on port 3000...\n");
    axum::serve(listener, app).await.unwrap();
    Ok(())
}

async fn root() -> impl IntoResponse {
    println!("/ GET <--");
    (StatusCode::OK, "Hello, World!".to_string());
}

fn cool_shortener(long_url: &String) -> String {
    let mut s = DefaultHasher::new();
    long_url.hash(&mut s);
    format!("{:x}", s.finish())
}

/// C -> S : shorten(long_url) ... S -> C : success(short_code)
async fn shorten(
    State(ctx): State<AppCtx>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(long_url) = params.get("q").map(|q| q.to_owned()) else {
        println!("/shorten POST <--");
        return (StatusCode::BAD_REQUEST, "URL was not provided".to_owned());
    };

    println!("/shorten POST <-- {}", &long_url);

    {
        // acquire lock
        let long_to_short_cache = ctx.long_to_short_cache.lock().unwrap();
        match long_to_short_cache.get(&long_url) {
            Some(short_code) => {
                println!("\tfound in cache");
                // already in cache, means already in db, can just return
                return (StatusCode::OK, short_code.to_owned());
            }
            None => {
                println!("\tcache miss - new entry");
                // not in cache, means its a new entry
            }
        }
        // lock is released
    }

    // not in cache, so add it
    let short_code = cool_shortener(&long_url);
    println!("\tshortened to: {}", &short_code);

    let url = URL {
        long_url: long_url.clone(),
        short_code: short_code.clone(),
    };

    match store_entry(url, &ctx.pool).await {
        Ok(_) => {
            {
                // acquire lock
                let mut long_to_short_cache = ctx.long_to_short_cache.lock().unwrap();
                long_to_short_cache.insert(long_url.clone(), short_code.clone());
                println!("\tstoring in lts cache");
                // release lock
            }

            {
                // acquire lock
                let mut short_to_long_cache = ctx.short_to_long_cache.lock().unwrap();
                short_to_long_cache.insert(short_code.clone(), long_url.clone());
                println!("\tstoring in stl cache");
                // release lock
            }

            println!("\tsaved to db");
            (StatusCode::OK, short_code)
        }

        Err(e) => {
            eprintln!("Failed to store entry: {}", e);
            // in the window between lock release and acquisition
            // it's possible that another thread added the short code into the db
            // and so we are violating the uniqueness constraint

            // in that case we just check the cache again
            // to see if the other thread added the short code

            let long_to_short_cache = ctx.long_to_short_cache.lock().unwrap();
            if let Some(existing_code) = long_to_short_cache.get(&long_url) {
                println!("\tother thread already stored short code");
                return (StatusCode::OK, existing_code.to_owned());
            }

            // otherwise something else happened so we just return an error
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong on our end".to_owned(),
            )
        }
    }
}

/// C -> S : redirect(short_code) ...  S -> C : {
///     found(long_url),
///     not_found()
/// }
async fn redirect(State(ctx): State<AppCtx>, Path(short_code): Path<String>) -> Response {
    println!("/redirect GET <-- {}", short_code);

    {
        // acquire lock on stl
        let short_to_long_cache = ctx.short_to_long_cache.lock().unwrap();
        match short_to_long_cache.get(&short_code) {
            Some(long_url) => {
                println!("\tfound in cache");
                return Redirect::permanent(&long_url).into_response();
            }
            None => {
                println!("\tcache miss - looking in db");
                // not in cache, let's check db
            }
        }
        // release lock on stl
    }

    match lookup_entry(&short_code, &ctx.pool).await {
        Ok(Some(url)) => {
            println!("\tfound in db");
            {
                // acquire lock
                let mut short_to_long_cache = ctx.short_to_long_cache.lock().unwrap();
                short_to_long_cache.insert(short_code.clone(), url.long_url.clone());
                println!("\tstoring in stl cache");
                // release lock
            }
            return Redirect::permanent(&url.long_url).into_response();
        }

        Ok(None) => {
            println!("\tnot in db");
            (
                StatusCode::NOT_FOUND,
                "Short code not recognised".to_owned(),
            )
                .into_response()
        }

        Err(e) => {
            eprintln!("Failed to lookup entry: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong on our end".to_owned(),
            )
                .into_response()
        }
    }
}

/// S -> D : store(URL) . D -> S : ok() . D -> S : ok() . end,
async fn store_entry(url: URL, pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let long_url = &url.long_url;
    let short_code = &url.short_code;

    sqlx::query!(
        "INSERT INTO url (long_url, short_code) VALUES ($1, $2)",
        long_url,
        short_code
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// S -> D : lookup(short_code) . D -> S : {
///     not_found()
///     ok(URL)
/// }
async fn lookup_entry(
    short_code: &String,
    pool: &sqlx::SqlitePool,
) -> Result<Option<URL>, sqlx::Error> {
    let res = sqlx::query_as!(URL, "SELECT * FROM url WHERE short_code = $1", short_code)
        .fetch_optional(pool)
        .await?;

    Ok(res)
}
