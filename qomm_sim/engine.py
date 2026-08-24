"""One simulation arm = (request protocol) x (disclosure mechanism).

The protocol decides *who observes a request*; the disclosure mechanism decides
*what the public learns afterwards*. Both are varied independently, exactly as
the proposal's phase-3 comparison requires.

Leakage model per protocol (what a market maker or an outside observer sees at
request time):

    plain_rfq  asset, size, direction, wallet, time      (every queried maker)
    plain_rfm  asset, size, wallet, time                 (direction withheld)
    plain_rfs  asset, wallet, request window             (size withheld)
    qomm_*     nothing -- the request never reaches a maker

In every arm a settled trade is visible on chain (wallet, size, time). The
phase-1 design does not hide post-settlement information, so the arms differ in
what they reveal about *requests that did not execute* and about direction
before execution.
"""

from __future__ import annotations

import copy
import math
import random
from dataclasses import dataclass, field

from .disclosure import DisclosureMechanism, Release, WindowObservation
from .market import MarketMaker, ReferenceMarket, Request, SimConfig, size_bucket

PLAIN_PROTOCOLS = ("plain_rfq", "plain_rfm", "plain_rfs")
QOMM_PROTOCOLS = ("qomm_rfq", "qomm_rfm", "qomm_rfs")

MARKOUT_HORIZONS = {"markout_50ms": 1, "markout_1s": 20, "markout_10s": 200}

# Prior the user falls back on when no public information is available.
PRIOR_HALF_TICKS = 26.0
PRIOR_HALF_SD = 14.0
TIGHT_HALF_SD = 5.0
USER_SLACK_TICKS = 6


@dataclass
class RequestObservation:
    """What a market maker learned at request time, if anything."""

    step: int
    wallet: int
    entity: int
    size: int | None
    direction: int | None
    executed: bool


@dataclass
class Settlement:
    step: int
    wallet: int
    entity: int
    size: int
    direction: int
    price: int
    mm_id: int


@dataclass(frozen=True)
class Probe:
    """A rate-limited two-sided probe an attacker is allowed to send.

    Both sides at one fixed size, because the midpoint of a two-sided quote
    cancels the half spread and leaves exactly the inventory skew:

        (ask_i + bid_i) / 2 - m_t = invcoef_i * inv_i

    That is the sharpest inventory estimator available to a probing entity.
    """

    step: int
    size: int
    wallet: int
    entity: int


@dataclass
class ProbeResult:
    step: int
    size: int
    best_ask: int | None
    best_bid: int | None
    per_mm_quotes: dict[int, tuple[int, int]] | None   # only leaked by plain protocols
    per_mm_inventory: dict[int, int]
    true_net_inventory: int
    ref_mid: int


@dataclass
class ArmResult:
    protocol: str
    disclosure: str
    requests: int
    fills: int
    no_quote: int
    rejected: int
    user_cost_ticks: list[float]
    mm_pnl: dict[int, float]
    mm_markouts: dict[str, list[float]]
    quote_continuation: float
    releases: list[Release]
    release_errors: dict[str, list[float]]
    suppression_rate: float
    observations: list[RequestObservation]
    settlements: list[Settlement]
    truth: list[dict]
    windows: list[WindowObservation]
    epsilon_spent_max: float
    probe_results: list[ProbeResult] = field(default_factory=list)

    def summary(self) -> dict:
        cost = self.user_cost_ticks
        out = {
            "protocol": self.protocol,
            "disclosure": self.disclosure,
            "requests": self.requests,
            "fills": self.fills,
            "fill_rate": self.fills / self.requests if self.requests else 0.0,
            "no_quote_rate": self.no_quote / self.requests if self.requests else 0.0,
            "user_cost_mean_ticks": sum(cost) / len(cost) if cost else None,
            "user_cost_median_ticks": _median(cost),
            "mm_pnl_total_ticklots": sum(self.mm_pnl.values()),
            "mm_pnl_per_fill": (sum(self.mm_pnl.values()) / self.fills) if self.fills else None,
            "quote_continuation": self.quote_continuation,
            "suppression_rate": self.suppression_rate,
            "epsilon_spent_max": self.epsilon_spent_max,
        }
        for key, values in self.mm_markouts.items():
            out[f"mm_{key}_mean"] = (sum(values) / len(values)) if values else None
        for key, values in self.release_errors.items():
            out[f"release_{key}_mae"] = (sum(values) / len(values)) if values else None
        return out


def _median(values: list[float]) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    mid = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[mid]
    return 0.5 * (ordered[mid - 1] + ordered[mid])


class BeliefState:
    """A maker's estimate of the informed fraction, from own flow plus public info."""

    def __init__(self, prior: float = 0.30, prior_var: float = 0.05):
        self.prior = prior
        self.prior_var = prior_var
        self.adverse = 0.0
        self.count = 0.0

    def observe_fill(self, adverse: bool) -> None:
        # exponential forgetting keeps the estimate responsive
        decay = 0.98
        self.adverse = decay * self.adverse + (1.0 if adverse else 0.0)
        self.count = decay * self.count + 1.0

    def own_estimate(self) -> tuple[float, float]:
        if self.count < 1.0:
            return self.prior, self.prior_var
        phi = self.adverse / self.count
        var = max(1e-3, phi * (1 - phi) / self.count)
        return phi, var

    def combined(self, public: tuple[float | None, float]) -> float:
        own, own_var = self.own_estimate()
        pub, pub_var = public
        if pub is None or not math.isfinite(pub_var):
            return own
        w_own = 1.0 / own_var
        w_pub = 1.0 / pub_var
        return (own * w_own + pub * w_pub) / (w_own + w_pub)


class LeakagePolicy:
    """What one protocol lets a market maker learn at request time.

    This used to be a four-way branch in the middle of the arm loop, which meant
    the answer to "what does RFM hide?" lived 150 lines away from the answer to
    "what does RFS hide?". Here each protocol is one small class, and adding a
    protocol is adding a class rather than editing the loop.
    """

    name = "none"

    def observe(self, step: int, req: Request) -> RequestObservation | None:
        """The record a maker keeps, or None when the request never reaches one."""
        return None


class FullRequestVisible(LeakagePolicy):
    """RFQ: every queried maker sees the asset, the size and the direction."""

    name = "plain_rfq"

    def observe(self, step: int, req: Request) -> RequestObservation | None:
        return RequestObservation(step, req.wallet, req.entity, req.size,
                                  req.direction, False)


class DirectionWithheld(LeakagePolicy):
    """RFM: the maker quotes both sides, so it learns size but not direction."""

    name = "plain_rfm"

    def observe(self, step: int, req: Request) -> RequestObservation | None:
        return RequestObservation(step, req.wallet, req.entity, req.size, None, False)


class SizeWithheld(LeakagePolicy):
    """RFS: a stream, so the maker learns that this wallet is active and no more."""

    name = "plain_rfs"

    def observe(self, step: int, req: Request) -> RequestObservation | None:
        return RequestObservation(step, req.wallet, req.entity, None, None, False)


class NothingReaches(LeakagePolicy):
    """The query-oblivious arms: the request never reaches a maker at all."""

    name = "qomm"


_POLICIES = {policy.name: policy for policy in (
    FullRequestVisible(), DirectionWithheld(), SizeWithheld())}


def leakage_policy(protocol: str) -> LeakagePolicy:
    return _POLICIES.get(protocol, NothingReaches())


class WindowAccumulator:
    """The counters a disclosure window is built from.

    They were eight loose locals reset by hand at the boundary, which is exactly
    the shape where one gets forgotten. Keeping them together makes the reset one
    statement and the window's contents one object.
    """

    def __init__(self, start: int = 0) -> None:
        self.start = start
        self.requests: dict[int, int] = {}
        self.volume: dict[int, int] = {}
        self.signed: dict[int, int] = {}
        self.fills_by_entity: dict[int, int] = {}
        self.fills = 0
        self.total = 0
        self.no_quote = 0
        self.fills_by_bucket = [0, 0, 0]
        self.requests_by_bucket = [0, 0, 0]

    def saw_request(self, req: Request, bucket: int) -> None:
        self.total += 1
        self.requests_by_bucket[bucket] += 1
        self.requests[req.entity] = self.requests.get(req.entity, 0) + 1

    def saw_no_quote(self) -> None:
        self.no_quote += 1

    def saw_fill(self, req: Request, bucket: int, signed: int) -> None:
        self.fills += 1
        self.fills_by_entity[req.entity] = self.fills_by_entity.get(req.entity, 0) + 1
        self.fills_by_bucket[bucket] += 1
        self.volume[req.entity] = self.volume.get(req.entity, 0) + req.size
        self.signed[req.entity] = self.signed.get(req.entity, 0) + signed

    def observation(self, window: int, end_step: int, makers_in_band: int,
                    lots_in_band: int) -> WindowObservation:
        return WindowObservation(
            window=window, start_step=self.start, end_step=end_step,
            requests_by_entity=dict(self.requests),
            volume_by_entity=dict(self.volume),
            signed_volume_by_entity=dict(self.signed),
            fills_by_entity=dict(self.fills_by_entity),
            fills=self.fills, requests=self.total, no_quote=self.no_quote,
            liquidity_lots_in_band=lots_in_band, makers_in_band=makers_in_band,
            fills_by_bucket=tuple(self.fills_by_bucket),
            requests_by_bucket=tuple(self.requests_by_bucket),
        )


def _user_half_estimate(disclosure_name: str, release: Release | None) -> tuple[float, float]:
    """What the user believes the best achievable half spread is."""
    if disclosure_name == "A_none" or release is None:
        return PRIOR_HALF_TICKS, PRIOR_HALF_SD
    if disclosure_name == "B_threshold":
        if release.published:
            return PRIOR_HALF_TICKS * 0.75, TIGHT_HALF_SD * 1.6
        return PRIOR_HALF_TICKS * 1.15, PRIOR_HALF_SD
    if disclosure_name == "C_dp" and release.published:
        rate = release.fields.get("fill_rate")
        if rate is None:
            return PRIOR_HALF_TICKS, PRIOR_HALF_SD
        # a high observed fill rate implies quotes are close to the mid
        estimate = PRIOR_HALF_TICKS * (1.35 - 0.7 * min(1.0, max(0.0, rate)))
        return estimate, TIGHT_HALF_SD
    return PRIOR_HALF_TICKS, PRIOR_HALF_SD


class _Arm:
    """One arm's mutable state, so the loop can be read a step at a time.

    `run_arm` used to be one 240-line function holding twenty locals, which is
    more than a reader can carry while deciding whether a change is safe. The
    state lives here and each method answers one question: who saw it, what did
    it cost, was it accepted, what did it do to the book.
    """

    def __init__(self, cfg: SimConfig, market: ReferenceMarket,
                 requests: list[Request], makers: list[MarketMaker],
                 protocol: str, disclosure: DisclosureMechanism, seed: int,
                 probes: list[Probe] | None, reactive: bool,
                 max_retries: int, retry_delay: int):
        self.cfg = cfg
        self.market = market
        self.requests = requests
        self.protocol = protocol
        self.leakage = leakage_policy(protocol)
        self.disclosure = disclosure
        self.reactive = reactive
        self.max_retries = max_retries
        self.retry_delay = retry_delay
        self.rng = random.Random(seed)
        self.mms = [copy.deepcopy(m) for m in makers]
        self.beliefs = {m.mm_id: BeliefState() for m in self.mms}

        self.by_step: dict[int, list[Request]] = {}
        for req in requests:
            self.by_step.setdefault(req.step, []).append(req)
        self.retries: dict[int, int] = {}
        self.probes_by_step: dict[int, list[Probe]] = {}
        for probe in probes or []:
            self.probes_by_step.setdefault(probe.step, []).append(probe)

        self.observations: list[RequestObservation] = []
        self.settlements: list[Settlement] = []
        self.truth: list[dict] = []
        self.releases: list[Release] = []
        self.windows: list[WindowObservation] = []
        self.user_cost: list[float] = []
        self.markouts: dict[str, list[float]] = {k: [] for k in MARKOUT_HORIZONS}
        self.release_errors: dict[str, list[float]] = {"requests": [], "signed_volume": []}
        self.probe_results: list[ProbeResult] = []

        self.fills = self.no_quote = self.rejected = 0
        self.quoting_samples = self.quoting_active = 0
        self.last_release: Release | None = None
        self.window = WindowAccumulator()

    # --- what the venue currently believes about informed flow --------------
    @property
    def public(self) -> tuple[float | None, float]:
        if self.last_release is None:
            return None, float("inf")
        return self.disclosure.public_signal(self.last_release)

    def public_for(self, mm_id: int) -> tuple[float | None, float]:
        """What one maker gets, which need not be what every maker gets.

        A release that reaches everybody and a release that reaches one
        subscriber are different experiments and give different answers: the
        first lets every maker narrow together, so the surplus is competed away
        whatever the information was worth privately. Separating them is the
        only way to tell an information effect from a competition effect, and
        the two were being measured as one.
        """
        only = getattr(self.disclosure, "reaches", None)
        if only is not None and mm_id not in only:
            return None, float("inf")
        return self.public

    def _record(self, req: Request, step: int, executed: bool) -> None:
        self.truth.append({"step": step, "entity": req.entity, "wallet": req.wallet,
                           "size": req.size, "direction": req.direction,
                           "executed": executed, "informed": req.informed})

    # --- pricing ------------------------------------------------------------
    def _best_quote(self, req: Request, ref_mid: int) -> tuple[int, MarketMaker] | None:
        """The cheapest eligible maker, in the user's own units.

        Cost rather than price, so the same comparison serves both directions:
        a buyer minimises the ask and a seller maximises the bid, which is
        minimising its negation.
        """
        best_cost = None
        best_mm = None
        for mm in self.mms:
            if not mm.eligible(req.size):
                continue
            phi_hat = self.beliefs[mm.mm_id].combined(self.public_for(mm.mm_id))
            ask, bid = mm.quote(ref_mid, req.size, phi_hat)
            price = ask if req.direction == 0 else bid
            cost = price if req.direction == 0 else -price
            if best_cost is None or cost < best_cost:
                best_cost, best_mm = cost, mm
        if best_mm is None:
            return None
        return (best_cost if req.direction == 0 else -best_cost), best_mm

    def _accepts(self, req: Request, quote: int, ref_mid: int) -> bool:
        """The user's walk-away rule.

        An informed user is willing to pay up to the move it expects; an
        uninformed one prices off whatever the venue published, which is the
        only channel through which disclosure can change the flow.
        """
        if req.informed:
            edge = abs(req.signal)
            limit = ref_mid + edge if req.direction == 0 else ref_mid - edge
        else:
            half, _ = _user_half_estimate(self.disclosure.name, self.last_release)
            limit = (ref_mid + half + USER_SLACK_TICKS if req.direction == 0
                     else ref_mid - half - USER_SLACK_TICKS)
        return quote <= limit if req.direction == 0 else quote >= limit

    def _retry_later(self, req: Request, step: int) -> None:
        """A real desk does not abandon the trade; it comes back."""
        key = id(req)
        attempts = self.retries.get(key, 0)
        later = step + self.retry_delay
        if attempts < self.max_retries and later < self.cfg.steps:
            self.retries[key] = attempts + 1
            self.by_step.setdefault(later, []).append(req)

    def _execute(self, req: Request, quote: int, ref_mid: int, mm: MarketMaker,
                 step: int, bucket: int) -> None:
        self.fills += 1
        signed = req.size if req.direction == 0 else -req.size
        self.window.saw_fill(req, bucket, signed)
        cost_ticks = (quote - ref_mid) if req.direction == 0 else (ref_mid - quote)
        self.user_cost.append(float(cost_ticks))

        mm.inventory += -signed
        mm.fills += 1
        for name, horizon in MARKOUT_HORIZONS.items():
            future = self.market.mid[min(step + horizon, self.cfg.steps)]
            pnl = ((quote - future) * req.size if req.direction == 0
                   else (future - quote) * req.size)
            self.markouts[name].append(float(pnl) / req.size)
            if name == "markout_1s":
                mm.realized_pnl += pnl
                self.beliefs[mm.mm_id].observe_fill(pnl < 0)
        if abs(mm.inventory) >= mm.inv_limit:
            mm.quoting = False
        self.settlements.append(Settlement(step, req.wallet, req.entity, req.size,
                                           req.direction, quote, mm.mm_id))

    def _handle(self, req: Request, step: int, ref_mid: int) -> None:
        bucket = size_bucket(req.size)
        self.window.saw_request(req, bucket)
        seen = self.leakage.observe(step, req)

        priced = self._best_quote(req, ref_mid)
        if priced is None:
            self.no_quote += 1
            self.window.saw_no_quote()
            self._record(req, step, executed=False)
            if seen is not None:
                self.observations.append(seen)
            return

        quote, mm = priced
        if not self._accepts(req, quote, ref_mid):
            self.rejected += 1
            self._record(req, step, executed=False)
            if seen is not None:
                self.observations.append(seen)
            if self.reactive:
                self._retry_later(req, step)
            return

        self._execute(req, quote, ref_mid, mm, step, bucket)
        self._record(req, step, executed=True)
        if seen is not None:
            seen.executed = True
            self.observations.append(seen)

    # --- the attacker's probes, answered in every arm by design ------------
    def _answer_probe(self, probe: Probe, step: int, ref_mid: int) -> None:
        best_ask = best_bid = None
        per_mm: dict[int, tuple[int, int]] = {}
        for mm in self.mms:
            if not mm.eligible(probe.size):
                continue
            phi_hat = self.beliefs[mm.mm_id].combined(self.public_for(mm.mm_id))
            ask, bid = mm.quote(ref_mid, probe.size, phi_hat)
            per_mm[mm.mm_id] = (ask, bid)
            if best_ask is None or ask < best_ask:
                best_ask = ask
            if best_bid is None or bid > best_bid:
                best_bid = bid
        self.probe_results.append(ProbeResult(
            step=step, size=probe.size, best_ask=best_ask, best_bid=best_bid,
            per_mm_quotes=per_mm if self.protocol.startswith("plain") else None,
            per_mm_inventory={m.mm_id: m.inventory for m in self.mms},
            true_net_inventory=sum(m.inventory for m in self.mms),
            ref_mid=ref_mid))

    # --- between steps ------------------------------------------------------
    def _adapt_spreads(self) -> None:
        for mm in self.mms:
            realized = mm.realized_pnl / max(1, mm.fills)
            if realized < 0:
                mm.base_half = min(60, mm.base_half + 1)
            elif realized > 400 and mm.base_half > 4:
                mm.base_half -= 1

    def _unwind(self) -> None:
        for mm in self.mms:
            if mm.inventory:
                mm.inventory -= int(math.copysign(min(abs(mm.inventory), 3), mm.inventory))
            if not mm.quoting and abs(mm.inventory) < mm.inv_limit * 0.6:
                mm.quoting = True

    def _close_window(self, step: int) -> None:
        makers_in_band, lots_in_band = _depth_snapshot(
            self.mms, self.beliefs, self.market.mid[step], self.public)
        obs = self.window.observation(step // self.cfg.window_steps, step,
                                      makers_in_band, lots_in_band)
        self.windows.append(obs)
        release = self.disclosure.release(obs, self.rng)
        self.releases.append(release)
        self.last_release = release
        if release.published and release.mode == "C_dp":
            self.release_errors["requests"].append(
                abs(release.fields["noisy_requests"] - release.fields["exact_requests"]))
            self.release_errors["signed_volume"].append(
                abs(release.fields["noisy_signed_volume"]
                    - release.fields["exact_signed_volume"]))
        self.window = WindowAccumulator(start=step + 1)

    # --- the loop itself, now short enough to read -------------------------
    def run(self) -> ArmResult:
        cfg = self.cfg
        for step in range(cfg.steps):
            ref_mid = self.market.mid[step]
            for req in self.by_step.get(step, []):
                self._handle(req, step, ref_mid)
            for probe in self.probes_by_step.get(step, []):
                self._answer_probe(probe, step, ref_mid)
            if self.reactive and step % 400 == 399:
                self._adapt_spreads()
            self._unwind()
            self.quoting_samples += len(self.mms)
            self.quoting_active += sum(1 for m in self.mms if m.quoting)
            if (step + 1) % cfg.window_steps == 0:
                self._close_window(step)
        return self._result()

    def _result(self) -> ArmResult:
        epsilon_max = 0.0
        if hasattr(self.disclosure, "accountants"):
            epsilon_max = max((a.spent for a in self.disclosure.accountants.values()),
                              default=0.0)
        return ArmResult(
            protocol=self.protocol,
            disclosure=self.disclosure.name,
            requests=len(self.requests),
            fills=self.fills,
            no_quote=self.no_quote,
            rejected=self.rejected,
            user_cost_ticks=self.user_cost,
            mm_pnl={m.mm_id: m.realized_pnl for m in self.mms},
            mm_markouts=self.markouts,
            quote_continuation=(self.quoting_active / self.quoting_samples
                                if self.quoting_samples else 0.0),
            releases=self.releases,
            release_errors=self.release_errors,
            suppression_rate=(sum(1 for r in self.releases if not r.published)
                              / len(self.releases)) if self.releases else 0.0,
            observations=self.observations,
            settlements=self.settlements,
            truth=self.truth,
            windows=self.windows,
            epsilon_spent_max=epsilon_max,
            probe_results=self.probe_results,
        )


def run_arm(
    cfg: SimConfig,
    market: ReferenceMarket,
    requests: list[Request],
    makers: list[MarketMaker],
    protocol: str,
    disclosure: DisclosureMechanism,
    seed: int,
    probes: list[Probe] | None = None,
    reactive: bool = False,
    max_retries: int = 2,
    retry_delay: int = 40,
) -> ArmResult:
    """Run one arm. The work is in `_Arm`; this is the name callers know."""
    return _Arm(cfg, market, requests, makers, protocol, disclosure, seed,
                probes, reactive, max_retries, retry_delay).run()


def _depth_snapshot(
    mms: list[MarketMaker],
    beliefs: dict[int, BeliefState],
    ref_mid: int,
    public: tuple[float | None, float],
    band_bps: int = 5,
    probe_size: int = 100,
) -> tuple[int, int]:
    band = band_bps * ref_mid // 10_000
    count = 0
    lots = 0
    for mm in mms:
        if not mm.eligible(probe_size):
            continue
        phi_hat = beliefs[mm.mm_id].combined(public)
        ask, bid = mm.quote(ref_mid, probe_size, phi_hat)
        if abs(ask - ref_mid) <= band and abs(bid - ref_mid) <= band:
            count += 1
            lots += mm.max_qty
    return count, lots
