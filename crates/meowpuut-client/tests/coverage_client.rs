#![forbid(unsafe_code)]
//! Comprehensive branch coverage tests for `meowpuut-client` (`memory`, `backend`,
//! `engine`, `layout`, and `windows_adapter`).

use std::sync::Arc;

use meowpuut_client::backend::{compact_japanese, ChineseWithJapaneseBackend, ChoicePoolSoA};
use meowpuut_client::engine::{Engine, InputMode};
use meowpuut_client::layout::{
    candidate_frame, expanded_candidate_rows, CollapsedCandidateLayout, KeyboardLayoutMetrics, Rect,
};
use meowpuut_client::memory::ChoiceMemorySoA;
use meowpuut_client::windows_adapter::{
    WindowsKeyAction, WindowsSession, VK_BACK, VK_DOWN, VK_ESCAPE, VK_NEXT, VK_PRIOR, VK_RETURN,
    VK_SPACE, VK_UP,
};
use meowpuut_core::Lexicon;
use meowpuut_lexicon::parse_japanese_tsv;

const PY_TSV: &str = concat!(
    "MEOWPUUT-PINYIN-1\n",
    "de\tde5\t的\t的\t5500\n",
    "hao\thao3\t好\t好\t5200\n",
    "hen\then3\t很\t很\t5100\n",
    "keai\tke3ai4\t可愛\t可爱\t5400\n",
    "ma\tma5\t嗎\t吗\t5000\n",
    "ni\tni3\t你\t你\t5500\n",
    "ni\tni2\t泥\t泥\t4800\n",
    "ni\tni4\t逆\t逆\t4700\n",
    "ni\tni1\t妮\t妮\t4600\n",
    "ni\tni3\t擬\t拟\t4500\n",
    "ni\tni4\t膩\t腻\t4400\n",
    "ni\tni2\t尼\t尼\t4300\n",
    "ni\tni2\t霓\t霓\t4200\n",
    "ni\tni4\t匿\t匿\t4100\n",
    "ni\tni3\t妳\t妳\t4050\n",
    "ni\tni2\t猊\t猊\t4020\n",
    "nide\tni3de5\t你的\t你的\t5400\n",
    "nihao\tni3hao3\t你好\t你好\t5600\n",
    "shijie\tshi4jie4\t世界\t世界\t5400\n",
    "wo\two3\t我\t我\t5600\n",
    "woxihuan\two3xi3huan1\t我喜歡\t我喜欢\t5600\n",
    "xihuan\txi3huan1\t喜歡\t喜欢\t5500\n",
    "# context\n",
    "你好\t600\n",
);

const JP_TSV: &str = concat!(
    "MEOWPUUT-JAPANESE-1\n",
    "watashi\t私\t5500\n",
    "neko\t猫\t5200\n",
    "kawaii\t可愛い\t5300\n",
);

#[test]
fn test_memory_and_pool_all_branches() {
    let mut mem = ChoiceMemorySoA::new();
    mem.learn("", "");
    mem.learn("ni", "妳");
    mem.learn("ni", "你");
    mem.learn("hao", "好");
    mem.learn("hao", "好");
    assert!(mem.is_pinned("ni", "你"));
    assert!(!mem.is_pinned("ni", "妳"));
    assert!(!mem.is_pinned("other", "你"));
    assert!(mem.boost_for("好") >= 360);
    assert_eq!(mem.boost_for("未知"), 0);
    assert_eq!(mem.last_committed(), "好");
    mem.clear_context();
    assert_eq!(mem.bigram_bonus_for("好"), 0);
    mem.learn("ni", "你");
    assert!(mem.bigram_bonus_for("好") > 0);

    let data = mem.serialize();
    let mut restored = ChoiceMemorySoA::deserialize(&data);
    assert!(restored.is_pinned("ni", "你"));
    assert!(restored.boost_for("好") >= 360);
    assert_eq!(ChoiceMemorySoA::format_wal_event("ni", ""), "");
    let wal = ChoiceMemorySoA::format_wal_event("zaoan", "早安");
    let compacted_snap = restored.compact_with_wal(&wal);
    assert!(restored.is_pinned("zaoan", "早安"));
    assert!(compacted_snap.contains("早安"));

    let mut pool = ChoicePoolSoA::with_capacity(8);
    assert!(pool.is_empty());
    pool.push("你好", 80, 5000.0, 5, false);
    pool.push("你好", 90, 5200.0, 5, false);
    pool.push("你好", 70, 4000.0, 5, false);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool.tier[0], 90);

    let compacted = compact_japanese(&[
        ("".into(), 5000, 1),
        ("猫".into(), 4800, 4),
        ("猫".into(), 5200, 4),
    ]);
    assert_eq!(compacted.len(), 1);
    assert_eq!(compacted[0].1, 5200);
}

#[test]
fn test_backend_and_engine_all_branches() {
    let py = Arc::new(Lexicon::load(PY_TSV).unwrap());
    let jp = parse_japanese_tsv(JP_TSV).unwrap();
    let mut backend = ChineseWithJapaneseBackend::new(Arc::clone(&py), jp.clone());
    backend.type_keys("woxihuannidenekohenkeai");
    assert!(!backend.candidates().is_empty());
    assert!(backend.select(0).is_some());
    assert!(backend.select(999).is_none());

    backend.clear();
    backend.type_keys("neko");
    assert_eq!(backend.preedit(), "neko");
    assert!(backend.candidates().contains(&String::from("猫")));
    assert!(backend.select(0).is_some());
    // Selecting learned a pin ("neko" -> "猫"); re-typing hits pinned tier = 100
    backend.type_keys("neko");
    assert_eq!(backend.candidates()[0], "猫");
    backend.clear();
    backend.type_keys("nihao");
    assert!(backend.select(0).is_some());
    backend.type_keys("nihao");
    assert_eq!(backend.candidates()[0], "你好");

    let mut engine = Engine::new(py, jp);
    engine.phrases_mut().add("", "");
    engine.phrases_mut().add(" hello", "");
    engine.phrases_mut().add("招呼", "你好世界");
    engine.phrases_mut().add("nh", "你好朋友");
    assert!(engine.phrases_mut().lookup("").is_empty());
    assert!(!engine.phrases_mut().lookup("n").is_empty());

    // Shift tap tracker
    engine.shift_tracker_mut().on_shift_down(100);
    assert!(engine.shift_tracker_mut().on_shift_up(250));
    engine.shift_tracker_mut().on_shift_down(100);
    engine.shift_tracker_mut().on_other_key();
    assert!(!engine.shift_tracker_mut().on_shift_up(200));
    assert!(!engine.shift_tracker_mut().on_shift_up(300));

    assert_eq!(engine.mode(), InputMode::Chinese);
    for ch in "nihao".chars() {
        assert!(engine.handle_char(ch));
    }
    assert!(!engine.handle_char('中'));
    engine.browse(1);
    engine.browse(-1);
    let committed = engine.space().unwrap();
    assert_eq!(committed, "你好");
    // Post-commit associations should now be active
    let assoc_cands = engine.current_candidates();
    assert!(!assoc_cands.is_empty());
    let follow = engine.select_candidate(0).unwrap();
    assert!(!follow.is_empty());

    // Enter commits raw input
    engine.clear();
    engine.handle_char('n');
    engine.handle_char('i');
    assert!(engine.backspace());
    assert_eq!(engine.enter().as_deref(), Some("n"));
    assert!(engine.enter().is_none());

    // Toggle to English mode
    assert_eq!(engine.toggle_chinese_english(), InputMode::English);
    engine.browse(1);
    assert!(engine.select_candidate(999).is_none());
    for ch in "teh".chars() {
        assert!(engine.handle_char(ch));
    }
    assert!(!engine.handle_char('1'));
    let eng_snap = engine.snapshot();
    assert!(eng_snap.candidates.contains(&String::from("the")));
    assert!(engine.backspace());
    assert!(engine.select_candidate(0).is_some());
    assert!(!engine.backspace());
    engine.handle_char('a');
    assert_eq!(engine.enter().as_deref(), Some("a"));

    engine.set_simplified(true);
    assert_eq!(engine.toggle_chinese_english(), InputMode::Chinese);
    engine.handle_char('n');
    let ch_snap = engine.snapshot();
    assert_eq!(ch_snap.raw_input, "n");
    assert_eq!(ch_snap.preedit, "n");
    // Partial selection keeps remaining composition
    engine.clear();
    for ch in "nihaox".chars() {
        engine.handle_char(ch);
    }
    let _ = engine.select_candidate(0);
}

#[test]
fn test_layout_and_windows_adapter_all_branches() {
    let empty_layout = CollapsedCandidateLayout::compute(&[], 320.0);
    assert!(!empty_layout.can_expand);

    let cands: Vec<String> = vec![
        "你好".into(),
        "世界和平".into(),
        "我喜歡你的貓".into(),
        "測試".into(),
        "更多候選詞".into(),
        "abc123".into(),
    ];
    let collapsed = CollapsedCandidateLayout::compute(&cands, 240.0);
    assert!(collapsed.can_expand);
    assert!(collapsed.reserved_expand_width > 0.0);

    let rows = expanded_candidate_rows(&cands, 4);
    assert!(rows.len() >= 2);

    let screen = Rect { x: 0.0, y: 0.0, width: 1280.0, height: 800.0 };
    let f1 = candidate_frame(Rect { x: 1250.0, y: 400.0, width: 2.0, height: 18.0 }, (300.0, 44.0), screen);
    assert!(f1.x + f1.width <= screen.width);
    let f2 = candidate_frame(Rect { x: -20.0, y: 10.0, width: 2.0, height: 18.0 }, (300.0, 44.0), screen);
    assert!(f2.x >= screen.x);
    assert!(f2.y >= screen.y);

    let m_port = KeyboardLayoutMetrics::for_viewport(390.0, 844.0, false);
    let m_compact = KeyboardLayoutMetrics::for_viewport(375.0, 667.0, false);
    let m_ipad = KeyboardLayoutMetrics::for_viewport(820.0, 1180.0, false);
    let m_land = KeyboardLayoutMetrics::for_viewport(844.0, 390.0, true);
    assert!(m_ipad.key_height > m_port.key_height);
    assert!(m_port.key_height > m_compact.key_height);
    assert!(m_compact.key_height > m_land.key_height);

    let py = Arc::new(Lexicon::load(PY_TSV).unwrap());
    let mut win = WindowsSession::new(py);
    win.set_simplified(false);
    assert!(!win.can_handle_key(0x41, b'a', true, false));
    assert!(!win.can_handle_key(VK_SPACE, b' ', false, false));
    assert_eq!(win.handle_key(0x41, b'a', true, false), WindowsKeyAction::PassThrough);
    assert_eq!(win.handle_key(0x31, b'1', false, false), WindowsKeyAction::PassThrough);

    // Type 'n', 'i' to get > 7 candidates for pagination
    assert_eq!(win.handle_key(0x4E, b'n', false, false), WindowsKeyAction::UpdateComposition);
    assert_eq!(win.handle_key(0x49, b'i', false, false), WindowsKeyAction::UpdateComposition);
    assert!(win.is_composing());
    assert_eq!(win.preedit(), "ni");
    assert_eq!(win.candidate_page().len(), 7);

    // Navigate down across page boundary
    for _ in 0..7 {
        assert_eq!(win.handle_key(VK_DOWN, 0, false, false), WindowsKeyAction::UpdateComposition);
    }
    assert_eq!(win.page(), 1);
    assert_eq!(win.handle_key(VK_UP, 0, false, false), WindowsKeyAction::UpdateComposition);
    assert_eq!(win.page(), 0);
    assert_eq!(win.handle_key(VK_NEXT, 0, false, false), WindowsKeyAction::UpdateComposition);
    assert_eq!(win.page(), 1);
    assert_eq!(win.handle_key(VK_PRIOR, 0, false, false), WindowsKeyAction::UpdateComposition);
    assert_eq!(win.page(), 0);

    // Select with space
    let act_sp = win.handle_key(VK_SPACE, b' ', false, false);
    assert!(matches!(act_sp, WindowsKeyAction::Commit(_)));

    // Select with digit key '2'
    win.handle_key(0x4E, b'n', false, false);
    win.handle_key(0x49, b'i', false, false);
    let act = win.handle_key(0x32, b'2', false, false);
    assert!(matches!(act, WindowsKeyAction::Commit(_)));

    // Partial selection in WindowsSession (keeps composition)
    for b in b"nihaox" {
        win.handle_key(u32::from(*b), *b, false, false);
    }
    assert_eq!(win.handle_key(VK_SPACE, b' ', false, false), WindowsKeyAction::UpdateComposition);
    win.handle_key(VK_ESCAPE, 0x1B, false, false);

    // Backspace & Escape & Return
    win.handle_key(0x4E, b'n', false, false);
    win.handle_key(0x49, b'i', false, false);
    assert_eq!(win.handle_key(VK_BACK, 0x08, false, false), WindowsKeyAction::UpdateComposition);
    assert_eq!(win.handle_key(VK_BACK, 0x08, false, false), WindowsKeyAction::ClearComposition);

    win.handle_key(0x4E, b'n', false, false);
    assert_eq!(win.handle_key(VK_ESCAPE, 0x1B, false, false), WindowsKeyAction::ClearComposition);

    win.handle_key(0x4E, b'n', false, false);
    assert_eq!(win.handle_key(VK_RETURN, 0x0D, false, false), WindowsKeyAction::Commit("n".into()));
}
