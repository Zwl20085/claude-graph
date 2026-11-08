//! [`super::answer`] against a simulated Claude Code dialog that behaves as
//! verified with the real CLI (digits select and submit a single choice,
//! arrows move, Space toggles, Submit opens a review screen).

use super::*;
use crate::contract::{DialogKind, PromptOption};

struct Sim {
    labels: Vec<String>,
    multi: bool,
    text_option: Option<usize>,
    cursor: usize,
    checked: Vec<bool>,
    /// Submit row position in nav (multi).
    submit_at: Option<usize>,
    review: bool,
    open: bool,
    typed: String,
    keys: Vec<String>,
    /// The CLI ignores every key (an answer that does not land).
    deaf: bool,
    result: Option<String>,
}

impl Sim {
    fn single(labels: &[&str], text_option: Option<usize>) -> Self {
        Sim {
            labels: labels.iter().map(|l| l.to_string()).collect(),
            multi: false,
            text_option,
            cursor: 0,
            checked: vec![false; labels.len()],
            submit_at: None,
            review: false,
            open: true,
            typed: String::new(),
            keys: Vec::new(),
            deaf: false,
            result: None,
        }
    }

    fn multi(labels: &[&str]) -> Self {
        let n = labels.len();
        Sim {
            multi: true,
            submit_at: Some(n),
            ..Sim::single(labels, None)
        }
    }

    fn nav(&self) -> Vec<NavRow> {
        let mut nav: Vec<NavRow> = (0..self.labels.len()).map(NavRow::Option).collect();
        if let Some(at) = self.submit_at {
            nav.insert(at, NavRow::Submit);
        }
        nav
    }

    fn on_key(&mut self, k: &str) {
        self.keys.push(k.to_string());
        if self.deaf || !self.open {
            return;
        }
        let len = self.nav().len();
        if self.review {
            if k == "\r" {
                self.open = false;
            }
            return;
        }
        match k {
            "\x1b[B" => self.cursor = (self.cursor + 1).min(len - 1),
            "\x1b[A" => self.cursor = self.cursor.saturating_sub(1),
            " " => {
                if let NavRow::Option(i) = self.nav()[self.cursor] {
                    self.checked[i] = !self.checked[i];
                }
            }
            "\r" => match self.nav()[self.cursor] {
                NavRow::Submit => self.review = true,
                NavRow::Option(i) if Some(i) == self.text_option => {
                    self.result = Some(self.typed.clone());
                    self.open = false;
                }
                NavRow::Option(i) if self.multi => self.checked[i] = !self.checked[i],
                NavRow::Option(i) => {
                    self.result = Some(self.labels[i].clone());
                    self.open = false;
                }
            },
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && self.typed.is_empty() => {
                let i = usize::from(d.as_bytes()[0] - b'1');
                if i < self.labels.len() {
                    self.cursor = self
                        .nav()
                        .iter()
                        .position(|r| *r == NavRow::Option(i))
                        .unwrap();
                    if Some(i) != self.text_option && !self.multi {
                        self.result = Some(self.labels[i].clone());
                        self.open = false;
                    }
                }
            }
            text => {
                if Some(self.cursor) == self.text_option {
                    self.typed.push_str(text);
                }
            }
        }
    }

    fn screen(&self) -> ScreenDialog {
        if self.review {
            return ScreenDialog {
                kind: DialogKind::Choice,
                question: "Ready to submit your answers?".into(),
                header: None,
                options: ["Submit answers", "Cancel"]
                    .iter()
                    .enumerate()
                    .map(|(index, l)| PromptOption {
                        index,
                        label: l.to_string(),
                        detail: None,
                    })
                    .collect(),
                checked: vec![false, false],
                cursor: Some(NavRow::Option(0)),
                nav: vec![NavRow::Option(0), NavRow::Option(1)],
                multi: false,
                text_option: None,
                hinted: true,
            };
        }
        ScreenDialog {
            kind: DialogKind::Choice,
            question: "Q?".into(),
            header: None,
            options: self
                .labels
                .iter()
                .enumerate()
                .map(|(index, l)| PromptOption {
                    index,
                    label: l.clone(),
                    detail: None,
                })
                .collect(),
            checked: self.checked.clone(),
            cursor: Some(self.nav()[self.cursor]),
            nav: self.nav(),
            multi: self.multi,
            text_option: self.text_option,
            hinted: true,
        }
    }
}

impl DialogIo for Sim {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Halt> {
        let s = String::from_utf8_lossy(bytes).to_string();
        // Split escape sequences into single keys.
        let mut rest = s.as_str();
        while !rest.is_empty() {
            let n = if rest.starts_with('\x1b') {
                3
            } else if rest.len() > 1
                && !rest.as_bytes()[0].is_ascii_digit()
                && rest != "\r"
                && !rest.starts_with(' ')
            {
                rest.len()
            } else {
                1
            };
            let (k, r) = rest.split_at(n.min(rest.len()));
            self.on_key(k);
            rest = r;
        }
        Ok(())
    }

    async fn wait(&mut self, _d: Duration) -> Result<(), Halt> {
        Ok(())
    }

    fn dialog(&self) -> Option<ScreenDialog> {
        self.open.then(|| self.screen())
    }
}

async fn run(sim: &mut Sim, a: PromptAnswer, style: &DialogStyle) -> Answered {
    let d = sim.screen();
    match answer(sim, &d, &a, style, false).await {
        Ok(r) => r,
        Err(e) => panic!("{e:?}"),
    }
}

#[tokio::test]
async fn digit_selects_and_submits_a_single_choice() {
    let mut sim = Sim::single(&["A", "B", "Type something.", "Chat"], Some(2));
    let r = run(&mut sim, PromptAnswer::Choice(1), &DialogStyle::claude()).await;
    assert_eq!(r, Answered::Changed);
    assert_eq!(sim.result.as_deref(), Some("B"));
    assert_eq!(sim.keys, ["2"], "no Enter once the dialog closed");
}

#[tokio::test]
async fn arrows_and_enter_without_digits() {
    let mut sim = Sim::single(&["Yes", "Always", "No"], None);
    sim.cursor = 1;
    let r = run(&mut sim, PromptAnswer::Choice(2), &DialogStyle::generic()).await;
    assert_eq!(r, Answered::Changed);
    assert_eq!(sim.result.as_deref(), Some("No"));
    assert_eq!(sim.keys, ["\x1b[B", "\r"]);
    let mut sim = Sim::single(&["Yes", "Always", "No"], None);
    sim.cursor = 2;
    run(&mut sim, PromptAnswer::Choice(0), &DialogStyle::generic()).await;
    assert_eq!(sim.keys, ["\x1b[A", "\x1b[A", "\r"]);
}

#[tokio::test]
async fn free_text_goes_through_the_text_option() {
    let mut sim = Sim::single(&["A", "B", "Type something.", "Chat"], Some(2));
    let r = run(
        &mut sim,
        PromptAnswer::Text("use C\nnow".into()),
        &DialogStyle::claude(),
    )
    .await;
    assert_eq!(r, Answered::Changed);
    assert_eq!(sim.result.as_deref(), Some("use C now"));
    let mut sim = Sim::single(&["A", "B"], None);
    let d = sim.screen();
    let err = answer(
        &mut sim,
        &d,
        &PromptAnswer::Text("x".into()),
        &DialogStyle::claude(),
        false,
    )
    .await;
    assert!(matches!(err, Err(AnswerError::Invalid(_))));
    assert!(sim.keys.is_empty());
}

#[tokio::test]
async fn multi_select_toggles_submits_and_confirms_the_review() {
    let mut sim = Sim::multi(&["X", "Y", "Z", "Type something"]);
    let r = run(
        &mut sim,
        PromptAnswer::Choices(vec![0, 2]),
        &DialogStyle::claude(),
    )
    .await;
    assert_eq!(r, Answered::Changed);
    assert!(!sim.open);
    assert_eq!(sim.checked, [true, false, true, false]);
    assert_eq!(
        sim.keys,
        [" ", "\x1b[B", "\x1b[B", " ", "\x1b[B", "\x1b[B", "\r", "\r"]
    );
}

#[tokio::test]
async fn an_answer_that_does_not_land_is_reported() {
    let mut sim = Sim::single(&["A", "B"], None);
    sim.deaf = true;
    let r = run(&mut sim, PromptAnswer::Choice(1), &DialogStyle::claude()).await;
    assert_eq!(r, Answered::Unchanged);
    assert!(sim.keys.len() <= 3, "{:?}", sim.keys);
}

#[test]
fn arrow_counts() {
    assert_eq!(arrows(0, 2), b"\x1b[B\x1b[B");
    assert_eq!(arrows(3, 1), b"\x1b[A\x1b[A");
    assert!(arrows(1, 1).is_empty());
}
