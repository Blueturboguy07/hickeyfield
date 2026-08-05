fn main() {
    let reg = halation_core::registry::registry();
    for m in reg.values() {
        for r in &m.routes {
            if r.provider == halation_core::ProviderId::Fal {
                println!("{}\t{}", m.id, r.slug);
            }
        }
    }
}
