fn main() {
    let reg = hickeyfield_core::registry::registry();
    for m in reg.values() {
        for r in &m.routes {
            if r.provider == hickeyfield_core::ProviderId::Fal {
                println!("{}\t{}", m.id, r.slug);
            }
        }
    }
}
