from pathlib import Path
b=Path.cwd()
p=b/'crates/nessa-client-core/src/retained.rs';s=p.read_text();old='''        let end = read_end(result, stop.is_stopped(), ending.spent(), progressed);''';assert old in s
s=s.replace(old,'''        eprintln!("N997 read result={result:?} stopped={} spent={} progressed={progressed}", stop.is_stopped(), ending.spent());
'''+old);p.write_text(s)
p=b/'crates/nessa-server/tests/composition/read_only_online/fixtures/gateway.rs';s=p.read_text();needle='''                if self.mode == "append" && count == 1 {''';assert needle in s
s=s.replace(needle,'''                if self.mode == "epoch" && count == 1 {
                    eprintln!("record fixture N997 first page source read and authority change complete");
                    let delay = std::env::var("NESSA_997_DELAY_FIRST_PAGE_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                    if delay > 0 { tokio::time::sleep(Duration::from_millis(delay)).await; }
                    eprintln!("record fixture N997 first page returning after delay_ms={delay}");
                }
'''+needle);p.write_text(s)
p=b/'crates/nessa-server/tests/composition/read_only_online/native_session.rs';s=p.read_text();needle='''    assert_eq!(refresh.read().unwrap().result(), &Err(ReadFailure::Refused));''';assert needle in s
s=s.replace(needle,'''    eprintln!("N997 refresh result={:?} recheck={}", refresh.read().unwrap().result(), refresh.recheck().is_some());
    let diagnostic_conversation = ConversationId::new(&setup.conversation).unwrap();
    match native.transcript(&diagnostic_conversation, Uuid::nil()) {
        Ok(RetainedTranscript::Retained(value)) => eprintln!("N997 saved downloaded={} applied={} facts={}", value.progress().downloaded(), value.progress().applied(), value.progress().facts()),
        Ok(_) => eprintln!("N997 no retained transcript"),
        Err(error) => eprintln!("N997 transcript error={error:?}"),
    }
'''+needle);p.write_text(s)
