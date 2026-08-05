//! Run the real enhancer against a local model and print what it does.
use halation_core::corpus;
use halation_core::enhancer::{enhance_or_original, mode_for, EnhanceRequest, LocalEnhancer};
use halation_core::{catalog::Modality, enhance::JobType};

fn main() {
    let tag = std::env::args().nth(1).unwrap_or("qwen2.5:7b".into());
    let prompt = std::env::args()
        .nth(2)
        .unwrap_or("a lighthouse in fog at dawn".into());

    let req = EnhanceRequest::new(&prompt, JobType::Video, "Kling 3.0", Modality::Video);
    let mode = mode_for(&req).unwrap();
    let system = corpus::system_prompt_for(mode).unwrap();
    println!("model:  {tag}");
    println!(
        "system: {} chars (~{} tokens)",
        system.len(),
        system.len() / 4
    );
    println!("in:     {prompt}\n");

    let t = std::time::Instant::now();
    let out = enhance_or_original(&LocalEnhancer::new(&tag, system), &req);
    println!("status: {:?}   {:?}", out.status, t.elapsed());
    if let Some(n) = &out.note {
        println!("note:   {n}");
    }
    println!("\nout:    {}", out.prompt);
}
