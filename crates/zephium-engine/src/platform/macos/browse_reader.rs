#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

//! The Ask panel's reader for a person's own Browse tab.
//!
//! Each view has one private content world holding a ~1 KB document-start
//! script. In every document it holds a single pull on its own handler and
//! does nothing else. Only when the person asks does native answer that pull
//! with the fixed program (the pinned semantic runtime plus the fixed
//! recipes), which the script instantiates inside the private world. Reads
//! and recipes then run as closed invocations over the pinned channel state
//! machine; WebKit's evaluation APIs are never used, so the page gains no
//! user activation.
//!
//! Every instantiation gets a fresh channel and handler registration, bound
//! to the committed navigation it was opened for. A navigation's commit ends
//! the previous document's channel and removes its handler, so a late answer
//! from an older document can never reach the newer one.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::hash::{BuildHasher as _, Hasher as _};
use std::panic::AssertUnwindSafe;
use std::ptr::null_mut;
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, DefinedClass as _, MainThreadOnly};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSString, NSUTF8StringEncoding};
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandlerWithReply, WKUserContentController,
    WKUserScript, WKUserScriptInjectionTime, WKWebView,
};
use zephium_agentic::{ContextJoin, SemanticRuntimeFault, SemanticRuntimeResultError};
use zephium_core::ids::ItemId;

use super::semantic_runtime::{
    browse_channel, AgentSemanticRuntimeController, AgentSemanticRuntimeDispatchError,
    AgentSemanticRuntimeFailure,
};
use crate::page_reader::{PageActFailure, PageReadFailure, PageReadout};
use crate::platform::page_recipe::PageRecipe;

const BOOT_HANDLER: &str = "zephiumPageAssistBootV1";
const CHANNEL_HANDLER: &str = "zephiumPageAssistV1";
const WORLD_PREFIX: &str = "zephium-page-assist-v1-";
/// A document's script has not received the program yet.
const BOOT_FRESH: &str = "B1";
/// A document's script holds the program and waits to serve again; the
/// token native gave it with the program follows.
const BOOT_INSTANTIATED: &str = "B2:";
const TOKEN_HEX: usize = 16;
/// The program could not be instantiated in this document.
const BOOT_FAILED: &str = "E1";
const RESTART: &str = "G1";
/// The program failed to start; pull once more for a retry.
const RETRY: &str = "Y1";
/// Instantiation attempts per document before it reads as unavailable.
const MAX_STARTS: u8 = 2;
/// Documents whose program token is remembered, for back-forward restores.
const MAX_DOCUMENTS: usize = 8;
const STOP: &str = "S1";
const FIXED_ERROR: &str = "zephium page reader refused";
const MAX_QUEUED: usize = 8;
const READ_TIMEOUT: Duration = Duration::from_secs(12);
const RECIPE_TIMEOUT: Duration = Duration::from_secs(5);
/// A channel unused this long is stopped; the document's script goes back to
/// its one dormant pull.
const IDLE: Duration = Duration::from_secs(180);

pub(crate) const BOOTSTRAP: &str = include_str!("../page_reader_bootstrap.js");
const KIT: &str = include_str!("../../host/picker_kit.js");
const RECIPES: &str = include_str!("../page_reader_recipes.js");
const SERVE: &str = include_str!("page_reader_serve.js");

/// The fixed program a document receives when the person first asks:
/// the pinned runtime installation, the shared picker, the recipes, and the
/// pull loop.
/// The program to send, or in the reader's probe one that fails to start.
fn first_program() -> &'static str {
    #[cfg(feature = "page-reader-probe")]
    if probe::take_broken_start() {
        return "throw 0;";
    }
    program()
}

#[cfg(feature = "page-reader-probe")]
pub(crate) mod probe {
    use std::sync::atomic::{AtomicU8, Ordering};

    static BROKEN_STARTS: AtomicU8 = AtomicU8::new(0);

    /// The next `count` program starts fail, as a page would make them.
    pub(crate) fn break_starts(count: u8) {
        BROKEN_STARTS.store(count, Ordering::Release);
    }

    pub(super) fn take_broken_start() -> bool {
        BROKEN_STARTS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }
}

pub(crate) fn program() -> &'static str {
    static PROGRAM: OnceLock<String> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        let installation = zephium_agentic::SEMANTIC_RUNTIME_PROGRAM.installation_source();
        let mut program =
            String::with_capacity(installation.len() + KIT.len() + RECIPES.len() + SERVE.len() + 3);
        program.push_str(installation);
        program.push('\n');
        program.push_str(KIT);
        program.push('\n');
        program.push_str(RECIPES);
        program.push('\n');
        program.push_str(SERVE);
        program
    })
}

type ReplyBlock = RcBlock<dyn Fn(*mut AnyObject, *mut NSString)>;
pub(crate) type ReadDone = Box<dyn FnOnce(Result<PageReadout, PageReadFailure>) + Send>;
pub(crate) type RecipeDone = Box<dyn FnOnce(Option<String>) + Send>;
pub(crate) type ActDone = Box<dyn FnOnce(Result<(), PageActFailure>) + Send>;

/// How long after a step the person's use of the page still pauses the
/// steps, and how often the page is asked about it meanwhile.
const LEASE: Duration = Duration::from_secs(15);
const LEASE_POLL: Duration = Duration::from_millis(200);
const ACT_TIMEOUT: Duration = Duration::from_secs(15);

fn monotonic_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}

enum Op {
    Read {
        epoch: u64,
        url: String,
        done: ReadDone,
    },
    Recipe {
        epoch: u64,
        recipe: PageRecipe,
        done: RecipeDone,
    },
    Act {
        epoch: u64,
        read: u64,
        url: String,
        proposal: zephium_agentic::SemanticActionProposal,
        done: ActDone,
    },
}

impl Op {
    fn fail(self, failure: PageReadFailure) {
        match self {
            Self::Read { done, .. } => done(Err(failure)),
            Self::Recipe { done, .. } => done(None),
            Self::Act { done, .. } => done(Err(match failure {
                PageReadFailure::PageChanged => PageActFailure::PageChanged,
                PageReadFailure::NotReady | PageReadFailure::Unavailable => PageActFailure::Failed,
            })),
        }
    }

    const fn epoch(&self) -> u64 {
        match self {
            Self::Read { epoch, .. } | Self::Recipe { epoch, .. } | Self::Act { epoch, .. } => {
                *epoch
            }
        }
    }
}

/// A document's dormant pull, waiting for the person to ask.
struct Pull {
    reply: ReplyBlock,
    /// The committed navigation it belongs to; unknown when it arrived
    /// while a navigation was under way.
    epoch: Option<u64>,
    /// The token of a document that already holds the program.
    token: Option<String>,
}

/// A document that received the program: its token, its address, and the
/// committed navigation it shows now.
struct Document {
    token: String,
    url: Option<String>,
    epoch: u64,
}

struct Channel {
    controller: AgentSemanticRuntimeController,
    epoch: u64,
    broken: Rc<Cell<bool>>,
    last_used: Instant,
}

/// The operation in flight: what its timer may end.
struct Current {
    id: u64,
    read: Option<zephium_agentic::SemanticInvocationId>,
    _timer: Option<super::ContentPolicyTimeout>,
}

struct ReaderState {
    tab: Rc<Cell<ItemId>>,
    controller: Retained<WKUserContentController>,
    view: objc2::rc::Weak<WKWebView>,
    view_address: usize,
    world: Retained<WKContentWorld>,
    context: ContextJoin,
    enabled: bool,
    /// The boot handler is registered.
    listening: bool,
    script: Option<Retained<WKUserScript>>,
    committed: Option<u64>,
    committed_url: Option<String>,
    /// The navigation now under way was a back or forward move.
    back_forward_pending: bool,
    /// The last commit was a back or forward move.
    back_forward: bool,
    /// Failed program starts in the committed document.
    starts_failed: Option<(u64, u8)>,
    documents: VecDeque<Document>,
    pull: Option<Pull>,
    channel: Option<Channel>,
    sequence: u64,
    queue: VecDeque<Op>,
    busy: Rc<Cell<bool>>,
    current: Option<Current>,
    idle_timer: Option<super::ContentPolicyTimeout>,
    /// The committed document we drew marks or a picker on.
    overlay: Option<u64>,
    /// A document being left whose drawings are still to be cleared.
    leaving: Option<u64>,
    /// Steps hold the page until then; the person's use of it pauses them.
    lease_until: Option<Instant>,
    lease_timer: Option<super::ContentPolicyTimeout>,
    committed_now: Rc<dyn Fn() -> Option<u64>>,
    /// No navigation is under way.
    settled: Rc<dyn Fn() -> bool>,
    weak: Weak<RefCell<ReaderState>>,
}

/// Re-runs the reader's queue from a fresh main-thread turn.
fn schedule_pump(tab: ItemId) {
    let _ = crate::host::try_with(move |host| host.page_reader_pump(tab));
}

fn answer(reply: ReplyBlock, value: &str) -> bool {
    objc2::exception::catch(AssertUnwindSafe(|| {
        let value = NSString::from_str(value);
        let pointer = Retained::as_ptr(&value).cast_mut().cast::<AnyObject>();
        reply.call((pointer, null_mut()));
    }))
    .is_ok()
}

fn refuse(reply: ReplyBlock) {
    let _ = objc2::exception::catch(AssertUnwindSafe(|| {
        let error = NSString::from_str(FIXED_ERROR);
        reply.call((null_mut(), Retained::as_ptr(&error).cast_mut()));
    }));
}

fn random_hex() -> String {
    let state = std::collections::hash_map::RandomState::new();
    let mut hasher = state.build_hasher();
    hasher.write_u64(0x544f_4b31);
    format!("{:016x}", hasher.finish())
}

fn world_name() -> String {
    let state = std::collections::hash_map::RandomState::new();
    let mut high = state.build_hasher();
    high.write_u64(0x5a45_5048);
    let mut low = state.build_hasher();
    low.write_u64(0x4153_4b31);
    format!("{WORLD_PREFIX}{:016x}{:016x}", high.finish(), low.finish())
}

/// Whether this WebKit can remove one world's scripts without touching the
/// view's others. Without it the reader never installs.
fn supported(controller: &WKUserContentController) -> bool {
    // SAFETY: `respondsToSelector:` is NSObject API on a live controller.
    unsafe {
        msg_send![
            controller,
            respondsToSelector: sel!(_removeAllUserScriptsAssociatedWithContentWorld:)
        ]
    }
}

fn remove_handler(controller: &WKUserContentController, world: &WKContentWorld, name: &str) {
    // SAFETY: main-thread controller and world are live retained objects;
    // exceptions are contained.
    let _ = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        controller.removeScriptMessageHandlerForName_contentWorld(&NSString::from_str(name), world);
    }));
}

fn map_failure(failure: &AgentSemanticRuntimeFailure) -> PageReadFailure {
    match failure {
        AgentSemanticRuntimeFailure::Dispatch(
            AgentSemanticRuntimeDispatchError::NotReady | AgentSemanticRuntimeDispatchError::Busy,
        ) => PageReadFailure::NotReady,
        AgentSemanticRuntimeFailure::DocumentReplaced | AgentSemanticRuntimeFailure::Retired => {
            PageReadFailure::PageChanged
        }
        AgentSemanticRuntimeFailure::Result(SemanticRuntimeResultError::Runtime(
            SemanticRuntimeFault::DocumentLoading,
        )) => PageReadFailure::NotReady,
        AgentSemanticRuntimeFailure::Dispatch(_)
        | AgentSemanticRuntimeFailure::Cancelled
        | AgentSemanticRuntimeFailure::RendererLost
        | AgentSemanticRuntimeFailure::TimedOut
        | AgentSemanticRuntimeFailure::Transport
        | AgentSemanticRuntimeFailure::Result(_) => PageReadFailure::Unavailable,
    }
}

impl ReaderState {
    fn tab(&self) -> ItemId {
        self.tab.get()
    }

    fn publish(&self) {
        let Some(epoch) = self.committed else {
            return;
        };
        // A document whose program would not start reads as unavailable
        // when asked, never as loading.
        let ready = self.enabled
            && (self.unstartable(epoch)
                || self
                    .pull
                    .as_ref()
                    .is_some_and(|pull| pull.epoch == Some(epoch))
                || self
                    .channel
                    .as_ref()
                    .is_some_and(|channel| channel.epoch == epoch));
        crate::page_reader::publish_ready(self.tab(), epoch, ready);
    }

    fn listen(&mut self) {
        if self.listening {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let handler_name = NSString::from_str(BOOT_HANDLER);
        let boot = BootHandler::alloc(mtm).set_ivars(BootIvars {
            controller: self.controller.clone(),
            world: self.world.clone(),
            name: handler_name.clone(),
            view: self.view_address,
            reader: self.weak.clone(),
        });
        // SAFETY: a freshly allocated instance of the declared class with
        // initialized ivars; `init` is NSObject's designated initializer.
        let boot: Retained<BootHandler> = unsafe { msg_send![super(boot), init] };
        // SAFETY: main thread proven above; every argument is retained for
        // the call and WebKit retains the handler. Exceptions are contained.
        self.listening = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            self.controller
                .addScriptMessageHandlerWithReply_contentWorld_name(
                    objc2::runtime::ProtocolObject::from_ref(&*boot),
                    &self.world,
                    &handler_name,
                );
        }))
        .is_ok();
    }

    /// The bootstrap present (for the next document) or absent.
    fn set_script(&mut self, present: bool) {
        if present == self.script.is_some() {
            return;
        }
        if !present {
            self.script = None;
            // SAFETY: `supported` proved the selector before this reader
            // existed; it takes one retained WKContentWorld, ours alone.
            let _ = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
                let _: () = msg_send![
                    &*self.controller,
                    _removeAllUserScriptsAssociatedWithContentWorld: &*self.world
                ];
            }));
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let source = NSString::from_str(BOOTSTRAP);
        // SAFETY: main thread proven above; source and world are retained
        // and WebKit retains the script. Exceptions become no script.
        self.script = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                WKUserScript::alloc(mtm),
                &source,
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
                &self.world,
            );
            self.controller.addUserScript(&script);
            script
        }))
        .ok();
    }

    fn stop_pull(&mut self) {
        if let Some(pull) = self.pull.take() {
            let _ = answer(pull.reply, STOP);
        }
    }

    /// Ends the document's channel; its handler goes with it, so nothing
    /// more from that document can reach the next one's.
    fn stop_channel(&mut self) {
        if let Some(channel) = self.channel.take() {
            channel.controller.cancel();
            remove_handler(&self.controller, &self.world, CHANNEL_HANDLER);
        }
    }

    fn unstartable(&self, epoch: u64) -> bool {
        self.starts_failed == Some((epoch, MAX_STARTS))
    }

    /// Binds a pull that arrived while a navigation was under way, once it
    /// settled. A document that already holds the program is only the
    /// committed one when it was committed there, or when a back or forward
    /// move just restored it at its own address; anything else is stale.
    fn bind_pull(&mut self) {
        let (Some(committed), true) = (self.committed, (self.settled)()) else {
            return;
        };
        let Some(pull) = self.pull.as_ref().filter(|pull| pull.epoch.is_none()) else {
            return;
        };
        let bound = match pull.token.as_deref() {
            None => true,
            Some(token) => match self.documents.iter_mut().find(|d| d.token == token) {
                Some(document) if document.epoch == committed => true,
                Some(document)
                    if self.back_forward
                        && document.url.is_some()
                        && document.url == self.committed_url =>
                {
                    document.epoch = committed;
                    true
                }
                _ => false,
            },
        };
        if bound {
            if let Some(pull) = self.pull.as_mut() {
                pull.epoch = Some(committed);
            }
        } else {
            self.stop_pull();
        }
    }

    /// A navigation begins: what we drew on the document goes with it,
    /// unless it only moves to a fragment of the same document.
    fn attempt(&mut self, admitted: bool, target: &str) {
        self.set_script(self.enabled && admitted);
        let document = |url: &str| url.split('#').next().unwrap_or(url).to_owned();
        if target.contains('#')
            && self
                .committed_url
                .as_deref()
                .is_some_and(|url| document(url) == document(target))
        {
            return;
        }
        if let Some(epoch) = self
            .overlay
            .take()
            .filter(|epoch| self.committed == Some(*epoch))
        {
            self.leaving = Some(epoch);
            self.pump();
        }
    }

    /// Clears what we drew on the document being left. It runs while the
    /// next navigation is still pending: until the next commit the channel
    /// belongs to that document alone, and a clear only removes our nodes.
    fn clear_leaving(&mut self) {
        if self.busy.get() {
            return;
        }
        let Some(epoch) = self.leaving.take() else {
            return;
        };
        if !self.enabled || self.committed != Some(epoch) {
            return;
        }
        if let Some(controller) = self
            .channel
            .as_ref()
            .filter(|channel| channel.epoch == epoch && !channel.broken.get())
            .map(|channel| channel.controller.clone())
        {
            self.dispatch(
                controller,
                Op::Recipe {
                    epoch,
                    recipe: PageRecipe::clear(),
                    done: Box::new(|_| {}),
                },
            );
        }
    }

    fn commit(&mut self, epoch: u64, url: &str) {
        self.committed = Some(epoch);
        self.lease_until = None;
        self.lease_timer = None;
        self.overlay = self.overlay.filter(|drawn| *drawn == epoch);
        self.leaving = self.leaving.filter(|drawn| *drawn == epoch);
        self.committed_url = Some(url.to_owned());
        self.back_forward = std::mem::take(&mut self.back_forward_pending);
        crate::page_reader::publish_commit(self.tab(), epoch);
        if self
            .channel
            .as_ref()
            .is_some_and(|channel| channel.epoch != epoch)
        {
            self.stop_channel();
        }
        if self
            .pull
            .as_ref()
            .is_some_and(|pull| pull.epoch.is_some_and(|bound| bound != epoch))
        {
            self.stop_pull();
        }
        self.bind_pull();
        self.publish();
    }

    fn boot_message(&mut self, body: &str, reply: ReplyBlock) {
        if !self.enabled {
            let _ = answer(reply, STOP);
            return;
        }
        if body == BOOT_FAILED {
            // The ask that started it ends unavailable now; the next ask on
            // this document tries once more, and then no more.
            let Some(epoch) = self.committed else {
                let _ = answer(reply, STOP);
                return;
            };
            let failed = match self.starts_failed {
                Some((bound, count)) if bound == epoch => count.saturating_add(1).min(MAX_STARTS),
                _ => 1,
            };
            self.starts_failed = Some((epoch, failed));
            if self
                .channel
                .as_ref()
                .is_some_and(|channel| channel.epoch == epoch)
            {
                self.stop_channel();
            }
            let _ = answer(reply, if failed < MAX_STARTS { RETRY } else { STOP });
            self.publish();
            return;
        }
        let token = body.strip_prefix(BOOT_INSTANTIATED).map(str::to_owned);
        // A fresh document is always the newest one. A document that already
        // holds the program never displaces another pull: it may be one being
        // left, whose loop just ended.
        if token.is_some() && self.pull.is_some() {
            let _ = answer(reply, STOP);
            return;
        }
        self.stop_pull();
        self.pull = Some(Pull {
            reply,
            epoch: None,
            token,
        });
        self.bind_pull();
        self.publish();
    }

    /// The document's live channel, or a fresh one opened by answering its
    /// dormant pull with the program (first time) or a restart.
    fn channel_for(
        &mut self,
        epoch: u64,
    ) -> Result<AgentSemanticRuntimeController, PageReadFailure> {
        if let Some(channel) = self
            .channel
            .as_mut()
            .filter(|channel| channel.epoch == epoch && !channel.broken.get())
        {
            channel.last_used = Instant::now();
            return Ok(channel.controller.clone());
        }
        self.stop_channel();
        if self.unstartable(epoch) {
            return Err(PageReadFailure::Unavailable);
        }
        let view = self.view.load().ok_or(PageReadFailure::PageChanged)?;
        self.bind_pull();
        let Some(pull) = self.pull.take_if(|pull| pull.epoch == Some(epoch)) else {
            return Err(PageReadFailure::NotReady);
        };
        let broken = Rc::new(Cell::new(false));
        let (on_invariant, on_panic) = (broken.clone(), broken.clone());
        let Ok(controller) = browse_channel(
            &self.controller,
            &self.world,
            &view,
            &NSString::from_str(CHANNEL_HANDLER),
            Rc::new(move || on_invariant.set(true)),
            Rc::new(move || on_panic.set(true)),
        ) else {
            self.pull = Some(pull);
            return Err(PageReadFailure::Unavailable);
        };
        // The first answer is a fresh token and the program; the token
        // names this document when it pulls again.
        let token = random_hex();
        let message = match pull.token {
            Some(_) => RESTART.to_owned(),
            None => format!("{token}\n{}", first_program()),
        };
        if !answer(pull.reply, &message) {
            controller.cancel();
            remove_handler(&self.controller, &self.world, CHANNEL_HANDLER);
            return Err(PageReadFailure::Unavailable);
        }
        if pull.token.is_none() {
            if self.documents.len() >= MAX_DOCUMENTS {
                self.documents.pop_front();
            }
            self.documents.push_back(Document {
                token,
                url: self.committed_url.clone(),
                epoch,
            });
        }
        self.channel = Some(Channel {
            controller: controller.clone(),
            epoch,
            broken,
            last_used: Instant::now(),
        });
        Ok(controller)
    }

    fn pump(&mut self) {
        self.clear_leaving();
        while !self.busy.get() {
            let Some(op) = self.queue.pop_front() else {
                return;
            };
            if !self.enabled {
                op.fail(PageReadFailure::Unavailable);
                continue;
            }
            let epoch = op.epoch();
            if (self.committed_now)() != Some(epoch) || self.committed != Some(epoch) {
                op.fail(PageReadFailure::PageChanged);
                continue;
            }
            match self.channel_for(epoch) {
                Ok(controller) => self.dispatch(controller, op),
                Err(failure) => op.fail(failure),
            }
        }
    }

    fn dispatch(&mut self, controller: AgentSemanticRuntimeController, op: Op) {
        let tab = self.tab();
        self.sequence = self.sequence.saturating_add(1);
        let id = self.sequence;
        let busy = self.busy.clone();
        busy.set(true);
        let finish = move || {
            busy.set(false);
            schedule_pump(tab);
        };
        let timeout = |limit| {
            super::schedule_content_policy_timeout(limit, move || {
                let _ = crate::host::try_with(move |host| host.page_reader_timeout(tab, id));
            })
        };
        match op {
            Op::Read { epoch, url, done } => {
                let Some((invocation, pending)) =
                    crate::page_reader::read::invocation(self.context, &url, id)
                else {
                    crate::page_reader::note(format_args!("reader: read failed at=invocation"));
                    done(Err(PageReadFailure::Unavailable));
                    finish();
                    return;
                };
                let read = invocation.invocation();
                let committed_now = self.committed_now.clone();
                let view = self.view.clone();
                let started = Instant::now();
                let dispatched = controller.dispatch(invocation, move |result| {
                    let outcome = match result {
                        Ok(_) if committed_now() != Some(epoch) => {
                            Err(PageReadFailure::PageChanged)
                        }
                        Ok(snapshot) => {
                            let captured = u64::try_from(started.elapsed().as_millis())
                                .unwrap_or(u64::MAX)
                                .max(1);
                            crate::page_reader::read::passages(pending, snapshot, captured, epoch)
                                .map(|(passages, kept)| {
                                    let read = crate::page_reader::publish_read(tab, kept);
                                    PageReadout {
                                        epoch,
                                        read,
                                        title: view.load().and_then(|view| title(&view)),
                                        passages,
                                    }
                                })
                        }
                        Err(_) if committed_now() != Some(epoch) => {
                            Err(PageReadFailure::PageChanged)
                        }
                        Err(failure) => {
                            crate::page_reader::note(format_args!(
                                "reader: read failed at=runtime failure={failure:?}"
                            ));
                            Err(map_failure(&failure))
                        }
                    };
                    done(outcome);
                    finish();
                });
                if dispatched.is_ok() {
                    self.current = Some(Current {
                        id,
                        read: Some(read),
                        _timer: timeout(READ_TIMEOUT),
                    });
                }
            }
            Op::Recipe {
                epoch,
                recipe,
                done,
            } => {
                let committed_now = self.committed_now.clone();
                self.current = Some(Current {
                    id,
                    read: None,
                    _timer: timeout(RECIPE_TIMEOUT),
                });
                controller.dispatch_recipe(recipe, move |answer| {
                    done(answer.filter(|_| committed_now() == Some(epoch)));
                    finish();
                });
            }
            Op::Act {
                epoch,
                read,
                url,
                proposal,
                done,
            } => {
                // The step is prepared against the page as it is now, then
                // runs as the runtime's fixed recipe; it is re-admitted at
                // the handoff only while the same document is committed.
                let Some((invocation, _)) =
                    crate::page_reader::read::invocation(self.context, &url, id)
                else {
                    done(Err(PageActFailure::Failed));
                    finish();
                    return;
                };
                self.current = Some(Current {
                    id,
                    read: None,
                    _timer: timeout(ACT_TIMEOUT),
                });
                let committed_now = self.committed_now.clone();
                let channel = controller.clone();
                let submits = crate::page_reader::read::submits(&proposal);
                let _ = controller.dispatch(invocation, move |result| {
                    let changed = || committed_now() != Some(epoch);
                    let current = match result {
                        Ok(_) if changed() => Err(PageActFailure::PageChanged),
                        Ok(snapshot) => Ok(snapshot),
                        Err(_) if changed() => Err(PageActFailure::PageChanged),
                        Err(_) => Err(PageActFailure::Failed),
                    };
                    let prepared = current.and_then(|snapshot| {
                        let stored = crate::page_reader::planned_read(tab, read)
                            .filter(|stored| stored.epoch() == epoch)
                            .ok_or(PageActFailure::PageChanged)?;
                        crate::page_reader::read::person_action(
                            &stored,
                            &snapshot,
                            proposal,
                            id,
                            monotonic_ms(),
                        )
                    });
                    let (pending, native) = match prepared {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            done(Err(failure));
                            finish();
                            return;
                        }
                    };
                    let still = committed_now.clone();
                    super::semantic_action::dispatch_closed(
                        &channel,
                        native,
                        Instant::now(),
                        Some(Box::new(move || still() == Some(epoch))),
                        move |settlement| {
                            let outcome = match pending.settle(settlement) {
                                Ok(()) => Ok(()),
                                Err(_) if committed_now() != Some(epoch) && submits => Ok(()),
                                Err(_) if committed_now() != Some(epoch) => {
                                    Err(PageActFailure::PageChanged)
                                }
                                Err(failure) => Err(crate::page_reader::read::act_failure(failure)),
                            };
                            done(outcome);
                            finish();
                        },
                    );
                });
            }
        }
        self.idle_timer = super::schedule_content_policy_timeout(IDLE, move || {
            let _ = crate::host::try_with(move |host| host.page_reader_idle(tab));
        });
    }

    fn timeout(&mut self, id: u64) {
        let Some(current) = self.current.take_if(|current| current.id == id) else {
            return;
        };
        let Some(channel) = self.channel.as_ref().filter(|_| self.busy.get()) else {
            return;
        };
        // A read the page never took ends timed out and the document stays
        // usable; anything it took may still answer, so the channel ends.
        let ended = match current.read {
            Some(read) => channel.controller.timeout(read),
            None => {
                channel.controller.cancel();
                true
            }
        };
        if ended {
            channel.broken.set(true);
        }
    }

    fn idle(&mut self) {
        self.idle_timer = None;
        if self.busy.get() {
            return;
        }
        if self
            .channel
            .as_ref()
            .is_some_and(|channel| channel.last_used.elapsed() >= IDLE)
        {
            self.stop_channel();
            self.publish();
        }
    }

    /// Asks the page whether the person used it, and keeps asking while
    /// steps hold it.
    fn poll_lease(&mut self) {
        self.lease_timer = None;
        let (Some(until), Some(epoch)) = (self.lease_until, self.committed) else {
            return;
        };
        if Instant::now() >= until {
            self.lease_until = None;
            return;
        }
        let tab = self.tab();
        if self.queue.len() < MAX_QUEUED {
            self.queue.push_back(Op::Recipe {
                epoch,
                recipe: PageRecipe::touched(),
                done: Box::new(move |answer| {
                    if answer.as_deref() == Some("1") {
                        crate::host::page_reader::report_human_input(tab);
                    }
                }),
            });
        }
        self.lease_timer = super::schedule_content_policy_timeout(LEASE_POLL, move || {
            let _ = crate::host::try_with(move |host| host.page_reader_poll(tab));
        });
        self.pump();
    }

    /// The script itself is added by the next admitted navigation attempt,
    /// so a document without a site (a blank spare) never gets it.
    fn enable(&mut self) {
        self.enabled = true;
        self.listen();
    }

    fn disable(&mut self) {
        self.enabled = false;
        self.lease_until = None;
        self.lease_timer = None;
        self.stop_pull();
        self.stop_channel();
        self.set_script(false);
        if self.listening {
            remove_handler(&self.controller, &self.world, BOOT_HANDLER);
            self.listening = false;
        }
        while let Some(op) = self.queue.pop_front() {
            op.fail(PageReadFailure::Unavailable);
        }
        self.idle_timer = None;
        self.publish();
    }
}

fn title(view: &WKWebView) -> Option<String> {
    // SAFETY: `title` is a main-thread WKWebView property read; objc2
    // retains the returned string.
    let title = unsafe { view.title() }?;
    if title.length() > 4 * 1024 {
        return None;
    }
    Some(zephium_core::item::sanitize_page_title(&title.to_string())).filter(|t| !t.is_empty())
}

/// One Browse view's reader. Lives with the view and is dropped before it.
pub(crate) struct BrowseReader {
    state: Rc<RefCell<ReaderState>>,
}

impl BrowseReader {
    /// A reader for `view`, or none when this WebKit cannot remove one
    /// world's scripts alone.
    pub(crate) fn new(
        view: &WKWebView,
        tab: Rc<Cell<ItemId>>,
        enabled: bool,
        committed_now: Rc<dyn Fn() -> Option<u64>>,
        settled: Rc<dyn Fn() -> bool>,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        // SAFETY: main thread proven above; objc2 retains the configuration
        // and its controller.
        let controller = unsafe { view.configuration().userContentController() };
        if !supported(&controller) {
            return None;
        }
        let name = NSString::from_str(&world_name());
        // SAFETY: `mtm` proves main-thread creation; `name` is a live
        // retained string and exceptions are contained.
        let world = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            WKContentWorld::worldWithName(&name, mtm)
        }))
        .ok()?;
        let context = crate::page_reader::read::reader_context()?;
        let state = Rc::new_cyclic(|weak| {
            RefCell::new(ReaderState {
                tab,
                controller,
                view: objc2::rc::Weak::new(view),
                view_address: std::ptr::from_ref(view).addr(),
                world,
                context,
                enabled: false,
                listening: false,
                script: None,
                committed: None,
                committed_url: None,
                back_forward_pending: false,
                back_forward: false,
                starts_failed: None,
                documents: VecDeque::new(),
                pull: None,
                channel: None,
                sequence: 0,
                queue: VecDeque::new(),
                busy: Rc::new(Cell::new(false)),
                current: None,
                idle_timer: None,
                overlay: None,
                leaving: None,
                lease_until: None,
                lease_timer: None,
                committed_now,
                settled,
                weak: weak.clone(),
            })
        });
        if enabled {
            state.borrow_mut().enable();
        }
        Some(Self { state })
    }

    /// A main-frame navigation to `target` is about to start: the next
    /// document gets the script, or none when `admitted` is false.
    pub(crate) fn attempt(&self, admitted: bool, target: &str) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.attempt(admitted, target);
        }
    }

    pub(crate) fn commit(&self, epoch: u64, url: &str) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.commit(epoch, url);
            state.pump();
        }
    }

    /// The main-frame navigation about to start is a back or forward move.
    pub(crate) fn back_forward(&self, back_forward: bool) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.back_forward_pending = back_forward;
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        match (enabled, state.enabled) {
            (true, false) => state.enable(),
            (false, true) => state.disable(),
            _ => {}
        }
    }

    fn enqueue(&self, op: Op) {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            op.fail(PageReadFailure::NotReady);
            return;
        };
        if state.queue.len() >= MAX_QUEUED {
            drop(state);
            op.fail(PageReadFailure::Unavailable);
            return;
        }
        state.queue.push_back(op);
        state.pump();
    }

    pub(crate) fn read(&self, epoch: u64, url: String, done: ReadDone) {
        self.enqueue(Op::Read { epoch, url, done });
    }

    pub(crate) fn recipe(&self, epoch: u64, recipe: PageRecipe, done: RecipeDone) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            if recipe.draws() {
                state.overlay = Some(epoch);
            } else if let Some(answer) = recipe
                .nothing_drawn()
                .filter(|_| state.overlay != Some(epoch))
            {
                drop(state);
                return done(Some(answer.to_owned()));
            }
        }
        self.enqueue(Op::Recipe {
            epoch,
            recipe,
            done,
        });
    }

    /// Runs one person-directed step on the document at `epoch`, and holds
    /// the page for a while: the person's own use of it is reported.
    pub(crate) fn act(
        &self,
        epoch: u64,
        read: u64,
        url: String,
        proposal: zephium_agentic::SemanticActionProposal,
        done: ActDone,
    ) {
        let start = match self.state.try_borrow_mut() {
            Ok(mut state) => {
                let start = state.lease_until.is_none();
                state.lease_until = Some(Instant::now() + LEASE);
                if start {
                    // Arms the page's listening before the step runs.
                    state.queue.push_back(Op::Recipe {
                        epoch,
                        recipe: PageRecipe::touched(),
                        done: Box::new(|_| {}),
                    });
                }
                start
            }
            Err(_) => false,
        };
        self.enqueue(Op::Act {
            epoch,
            read,
            url,
            proposal,
            done,
        });
        if start {
            self.poll();
        }
    }

    pub(crate) fn poll(&self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.poll_lease();
        }
    }

    pub(crate) fn pump(&self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.pump();
        }
    }

    pub(crate) fn timeout(&self, id: u64) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.timeout(id);
        }
    }

    pub(crate) fn idle(&self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.idle();
        }
    }

    /// Stops the channel now, as the idle timer would.
    #[cfg(feature = "page-reader-probe")]
    pub(crate) fn stop_now(&self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.stop_channel();
            state.publish();
        }
    }
}

impl Drop for BrowseReader {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.disable();
            crate::page_reader::forget(state.tab());
        }
    }
}

struct BootIvars {
    controller: Retained<WKUserContentController>,
    world: Retained<WKContentWorld>,
    name: Retained<NSString>,
    view: usize,
    reader: Weak<RefCell<ReaderState>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumPageAssistBootHandler"]
    #[ivars = BootIvars]
    struct BootHandler;

    // SAFETY: `define_class!` fixes `NSObject` as the superclass and objc2
    // initializes the declared ivars before exposing an instance.
    unsafe impl NSObjectProtocol for BootHandler {}

    // SAFETY: the selector and parameters exactly implement WebKit's
    // reply-handler protocol; `MainThreadOnly` keeps the object on the main
    // thread and callback state lives in ivars.
    unsafe impl WKScriptMessageHandlerWithReply for BootHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:replyHandler:))]
        unsafe fn user_content_controller_did_receive_script_message_reply_handler(
            &self,
            controller: &WKUserContentController,
            message: &WKScriptMessage,
            reply: &block2::DynBlock<dyn Fn(*mut AnyObject, *mut NSString)>,
        ) {
            let ivars = self.ivars();
            let reply = reply.copy();
            // SAFETY: WebKit supplied live callback objects for this exact
            // selector; objc2 retains each returned property.
            let (world, name, webview, frame) = unsafe {
                (
                    message.world(),
                    message.name(),
                    message.webView(),
                    message.frameInfo(),
                )
            };
            // SAFETY: `frame` is the live retained WKFrameInfo of this
            // message, read on WebKit's main-thread delivery path.
            let (frame_webview, main_frame) = unsafe { (frame.webView(), frame.isMainFrame()) };
            let same_view = |view: Option<&WKWebView>| {
                view.is_some_and(|view| std::ptr::from_ref(view).addr() == ivars.view)
            };
            if !std::ptr::eq(controller, &*ivars.controller)
                || Retained::as_ptr(&world) != Retained::as_ptr(&ivars.world)
                || !name.isEqualToString(&ivars.name)
                || !same_view(webview.as_deref())
                || !same_view(frame_webview.as_deref())
                || !main_frame
            {
                refuse(reply);
                return;
            }
            // SAFETY: `message` is live for this callback; objc2 retains its
            // body before the type and size checks.
            let body = unsafe { message.body() };
            let Ok(body) = body.downcast::<NSString>() else {
                refuse(reply);
                return;
            };
            let limit = BOOT_INSTANTIATED.len() + TOKEN_HEX;
            if body.length() > limit
                || body.lengthOfBytesUsingEncoding(NSUTF8StringEncoding) > limit
            {
                refuse(reply);
                return;
            }
            let body = body.to_string();
            let token_shaped = body.strip_prefix(BOOT_INSTANTIATED).is_some_and(|token| {
                token.len() == TOKEN_HEX && token.bytes().all(|b| b.is_ascii_hexdigit())
            });
            if body != BOOT_FRESH && body != BOOT_FAILED && !token_shaped {
                refuse(reply);
                return;
            }
            let Some(reader) = ivars.reader.upgrade() else {
                let _ = answer(reply, STOP);
                return;
            };
            let Ok(mut state) = reader.try_borrow_mut() else {
                let _ = answer(reply, STOP);
                return;
            };
            state.boot_message(&body, reply);
            let tab = state.tab();
            drop(state);
            schedule_pump(tab);
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_start_piece_stays_small_and_closed() {
        assert!(BOOTSTRAP.len() <= 1_024, "{}", BOOTSTRAP.len());
        assert!(BOOTSTRAP.is_ascii());
        assert_eq!(BOOTSTRAP.matches(BOOT_HANDLER).count(), 1);
        for forbidden in ["eval(", "document.", "fetch(", "XMLHttpRequest", "import("] {
            assert!(!BOOTSTRAP.contains(forbidden), "{forbidden}");
        }
        for token in [
            BOOT_FRESH,
            BOOT_INSTANTIATED,
            BOOT_FAILED,
            RESTART,
            STOP,
            RETRY,
        ] {
            assert!(BOOTSTRAP.contains(&format!("\"{token}\"")), "{token}");
        }
    }

    #[test]
    fn the_program_is_the_pinned_runtime_then_the_fixed_recipes() {
        let program = program();
        assert!(program.is_ascii());
        assert!(
            program.starts_with(zephium_agentic::SEMANTIC_RUNTIME_PROGRAM.installation_source())
        );
        assert_eq!(program.matches(RECIPES).count(), 1);
        assert_eq!(program.matches(KIT).count(), 1);
        assert!(program.find(KIT) < program.find(RECIPES));
        assert!(program.ends_with(SERVE));
        assert_eq!(SERVE.matches("handlers.zephiumPageAssistV1").count(), 1);
    }

    #[test]
    fn world_names_are_unpredictable() {
        let a = world_name();
        let b = world_name();
        assert_ne!(a, b);
        assert!(a.starts_with(WORLD_PREFIX) && a.len() == WORLD_PREFIX.len() + 32);
    }
}
