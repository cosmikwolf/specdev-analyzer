use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};
fn run(q: &str, src: &str) {
    let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut p = Parser::new(); p.set_language(&lang).unwrap();
    let t = p.parse(src, None).unwrap();
    let query = match Query::new(&lang, q) { Ok(q) => q, Err(e) => { println!("QUERY ERR {e}"); return; } };
    let mut c = QueryCursor::new();
    let mut ms = c.matches(&query, t.root_node(), src.as_bytes());
    while let Some(m) = ms.next() {
        let caps: Vec<String> = m.captures.iter().map(|c| format!("@{}={}", query.capture_names()[c.index as usize], &src[c.node.byte_range()].replace('\n'," "))).collect();
        println!("  pat{} {}", m.pattern_index, caps.join(" | "));
    }
}
#[test] #[ignore] fn q() {
    let src = "#[test]\n#[ignore]\nfn off() {}\n#[ignore]\n#[test]\nfn off2() {}\n#[cfg(x)]\n#[test]\n#[should_panic]\nfn sp() {}\nfn plain() {}\n#[tokio::test]\nasync fn a() {}\n";
    println!("A: quantified run");
    run(r#"
(
  (attribute_item (attribute [(identifier) (scoped_identifier)] @_t) (#match? @_t "^(test|rstest|([a-z_]+::)*test)$"))
  .
  (attribute_item)*
  .
  (function_item name: (identifier) @test.name) @test
)
"#, src);
    println!("B: ignore anywhere in run");
    run(r#"
(
  (attribute_item (attribute (identifier) @_i (#eq? @_i "ignore"))) @test.pending
  .
  (attribute_item)*
  .
  (function_item) @test
)
"#, src);
}
