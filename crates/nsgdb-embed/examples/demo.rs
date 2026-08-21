//! Demo nsgdb-embed — prova o seam sem rede.
//! Run: `cargo run -p nsgdb-embed --example demo`

use neural_sgdb::embedder::Embedder;
use neural_sgdb::{InMemory, Sgdb};
use nsgdb_embed::LocalEmbedder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let embedder = LocalEmbedder::default_384();
    let mut db = Sgdb::open(InMemory::new())?;

    let e1 = embedder.embed("clima ensolarado em sao paulo")?;
    db.remember_semantic("clima:sp", "clima ensolarado em sao paulo", &e1)?;

    let q = embedder.embed("sol no sudeste")?;
    let hits = db.recall(&q, 2)?;
    println!("[demo] hits: {}", hits.len());
    for h in &hits {
        println!("  - {} | {} d={:.3}", h.key, h.text, h.dist);
    }

    let wrong = LocalEmbedder::new(128).embed("sol")?;
    assert!(db.remember_semantic("bad", "x", &wrong).is_err());
    println!("[demo] era guard OK");
    Ok(())
}
