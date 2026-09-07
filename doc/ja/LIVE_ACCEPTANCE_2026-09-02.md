# 実機受入 2026-09-02: 法人キューの停止・再開、同時投入、選択的 abort

対象: OmenX 上の Docker デモ `qomm-e2e-20260901-final3`(DeFMI 5 検証者、MPC 7 ノード、Maker 4 社、Taker 1 社、Gateway)。
前セッションの完了報告のうち未実施だった項目を、ブラウザを使わず、機械可読な記録だけで判定した。

記録は `artifacts/live_acceptance/2026-09-02/` にある。1 シナリオにつき、開始前と終了後の全体スナップショット
(`<scenario>.00-before.json`, `<scenario>.99-after.json`)、途中の観測(`<scenario>.NN-*.json`)、Gateway と Taker
モジュールのログ、そして判定結果 `<scenario>.judge.json` を置く。判定は `qomm-live-acceptance judge` が記録ファイル
だけから再計算するので、ネットワークに触れずに再現できる。

## 使ったもの

- `rust/qomm-demo/src/bin/qomm_live_acceptance.rs`: デモネットワーク内で動く受入ドライバ。Gateway の WebSocket で
  Taker 席を取って RFQ を投げ(`rfq`)、Maker 席で方針を変え(`policy`)、Taker モジュールの outbox と DeFMI の hold を
  追い(`wait`)、7 ノードの health・常駐 Maker 状態・受理済み実行と DeFMI の pool note を記録し(`snapshot`)、記録から
  1シナリオの合否を出し(`judge`)、全シナリオの識別子・受領証・残高・ハッシュを1つの記録へまとめる
  (`report`)。署名も outbox ファイルの読み書きもしない。
- `demo-network/live_acceptance.sh`: Docker ホスト側でコンテナを止め、殺し、起こす手順。観測はすべて上のバイナリに任せる。
- ゲートウェイの変更 1 点(`rust/qomm-demo/src/distributed_mpc.rs`): 署名済み RFQ を法人 outbox へ入れる処理を、
  7 ノード全員を必要とする常駐 Maker 状態の突き合わせより前へ移した。委員会が落ちている間に署名された要求は、
  以前は突き合わせで失敗して一度も記録されなかった。今は outbox に `queued` のまま残り、委員会が戻ると
  Gateway の 500 ms 周期の再送ループが同じ署名バイト列を自動で再実行する。
- ゲートウェイの変更 2 点目(`rust/qomm-demo/src/room.rs`, `mpc.rs`, `distributed_mpc.rs`): ラウンドが「正本照合待ち」で
  終わると、Gateway は再実行に備えて Taker の予約を保持する。ところが `pool-sum` の 1 回目で、Gateway が用意した
  no-fill の返金遷移より先に Taker モジュール自身の解放を DeFMI が受理し(`DeFMI accepted a different Taker no-fill refund`)、
  outbox 項目は `released` で終端したのに Gateway 側の予約だけが残って、以後の全 RFQ を
  `the previous Taker reservation is still active` で拒み続けた(再送ループは終端した項目を claim しないので、
  誰も予約を解かない)。エンジンは照合待ちで終えた要求の id を覚え、再送ループが何も claim できないときに
  正本の状態(`/v1/outbox/reconcile`)を 5 秒に 1 回確かめ、`queue_finalized` かつ `released` なら
  Gateway の予約と残高の投影を解放してラウンドを `released`(`corporate_finalized`)として記録する。`consumed` で終端した
  未決済の要求は解放せず運用者向けに残す。`pool-sum` の 2 回目はこの修正を入れたイメージで動かした。
- `qomm-live-acceptance report`: 全シナリオの記録と `judge` の合否を 1 つの `acceptance.json` にまとめるRust実装。
  request id・digest・sequence・hold id・DeFMI 受領証、7 ノードの受理済み実行、前後の Taker 残高、pool sequence、
  ノード世代の会計、全記録ファイルの SHA-256 を持ち、`judge` とは独立に同じ不変条件(識別子の連鎖、hold と受領証の一致、
  pool の一歩前進、世代の会計)を再導出して食い違えば不合格にする。実装は
  `rust/qomm-demo/src/bin/qomm_live_acceptance_report.rs` に分離し、上記バイナリの `report` サブコマンドからだけ呼ぶ。
- `judge` の約定突き合わせ: pool id が 2 つのスナップショットで同じ束縛に残ることを仮定しない。7 ノードの
  `/v1/rounds` 受領証(round id・実行世代・入出力と persistence のダイジェスト)から残余 note の id を
  Gateway の `locate_accepted_execution` と同じ導出で再計算し、DeFMI の `currentPoolNoteID` と一致した pool を
  「その約定が引き当てた pool」とする。その pool について、7 ノードの部分コミットメントの和が現在の pool note の
  コミットメントに一致すること、各ノードが受領証の世代で実行し決済後にちょうど 1 回コミットしたこと
  (世代 = 受領証の世代 + 1)も判定する。ノード世代の動きは「新たに束縛した pool の数(ラウンド開始時に再登録された
  pool への登録開示値の再配布。DeFMI の sequence は動かず、経済的な意味はない)+ 約定 1 件につき 1 回のコミット」と
  一致することを全ノードに要求し、停止中に取った 2 つのスナップショットの間では世代も pool sequence も hold も
  動いていないことを別に確かめる。

## 1. 期限切れになった seq 7 は決済されていない

request id `3ff55a4e…9f4d`、digest `0a8c3215…7d1a`。受付 1788296631、署名期限 1788300219(受付 + 3590 s)。

- Taker outbox: `released`。受領証は同じ digest に束縛され、DeFMI 遷移 `bdd76cb8…554d`、高さ 159、finalized 1788305072。
- DeFMI: hold `f8ab6099…724d` は `released`、`settlementDigest` が上の遷移と一致。`consumed` になった痕跡はない。
- 解放は期限(1788300219)の後に起きた(1788305072)。期限前に解放する経路はない(`release_expired_taker` は
  `now > mandate.deadline` を要求する)。
- 同じ request id で別のバイト列を投入すると 409、期限が過ぎた `expires_at` で投入すると 409(`expired-seq7.02/03-*.txt`)。

期限切れの署名を再利用できない理由は次の 4 層にある。どれか 1 つでも十分で、互いに独立している。

1. **署名の中に期限がある。** Taker mandate は `deadline` を含めて Ed25519 で署名されている。Gateway は再送のたびに
   `QueuedRfqEnvelope::verify` で `deadline <= now` を拒否し、DeFMI の予約(`reserve_taker_note`)も同じ mandate を
   `verify(now)` で検査する。期限だけを書き換えた mandate は署名が壊れる。
2. **request id は署名された nullifier である。** `request_id = rfq_nullifier = H(participant ‖ slot)` は mandate の
   一部で、outbox はこの id を一度しか受け付けない(同 id・別バイト列は拒否、同 id・同バイト列は既存の状態を返す)。
   `released` の項目は `claim_next` が二度と返さない。
3. **予約(hold)は一度きりである。** hold id `H(TAKER-RESERVE ‖ participant ‖ slot)` も mandate に入っており、DeFMI 上で
   `released` になった hold は再び `active` にならない。同じ id での再予約は正本が拒否する。
4. **決済は予約を消費する遷移である。** `consumed` は `active` からしか遷移しない。`released` から決済へ進む遷移は
   状態機械に存在しない。

## 2. 委員会停止中に受け付けた署名済み要求の自動再実行

### 2a. 全ノードと Gateway を止めた状態で受け付け、`queued` のまま残す(`outage-queue`)

1. 7 ノードを止める(unix 1788328360)。Maker 席は manual に固定してある。
2. USD/JPY 1 単位の買い RFQ を投入する。Gateway は署名済み envelope を Taker outbox へ入れてから委員会の health を
   見て、`MPC nodes are unavailable; the signed RFQ is durably queued and no local execution was attempted` で
   ラウンドを止める(`01-rfq-while-down.json`、`abort_code: queued`)。
3. outbox の seq 11: request id `73b2e025…3691`、digest `660a3670…4a2b`、受付 1788328366、署名期限 1788331955。
   状態 `queued`、DeFMI に hold は無い(`02-queued.json`)。
4. Gateway も止める(1788328383)。7 ノードの `/health` と Gateway が全て不達の状態で Taker モジュールだけが
   seq 11 を保持している(`03-all-down.json`)。20 秒後も同じ(`04-still-queued.json`)。
5. ノードを起こし(1788328431)、Gateway を起こす(1788328441)。ブラウザも新しい署名も無い。Gateway の再送ループが
   seq 11 を claim し、同じ nullifier から予約 id `af9b84e6…b8df` で DeFMI に hold を作り、7 ノードで実行し、決済した。
   outbox は `settled`、受領証は DeFMI 遷移 `2422077a…`、高さ 207、finalized 1788328584(`05-replayed.json`)。
   DeFMI 側 hold の `settlementDigest` が同じ遷移を指す。
6. 終了後の outbox に seq 11 より後の項目は無い(別の要求で埋め合わせたのではない)。

投入から決済まで 218 秒。停止中の要求が「一度も記録されず拒否される」のではなく「記録されて後で実行される」ことが
この試験の中身であり、それを可能にしたのが前節のゲートウェイ変更である。

### 2b. DeFMI 予約の後、決済の前に Gateway を強制終了する(`outage-dispatching`)

seq 12(request `bc4702fd…`、digest `1fe10446…`)。`wait --until hold-active` が hold `c52348a4…9f3` の `active` を
DeFMI で観測した時点(高さ 219、outbox は `dispatching`)で `docker kill` した(1788329455)。Gateway 不達の間、
hold は `active` のまま、決済も解放も起きない(`03-gateway-down.json`)。Gateway を起こすと(1788329469)再送ループが
同じ seq・同じ hold で再実行し、`reserve_taker_note` は既存の hold を再利用(冪等)、決済は高さ 222、遷移 `9fddd29a…`。
予約は 1 つ、消費は 1 回。

## 3. 同じ pool・同じ法人枠を合計で超える 2 件の同時投入

### 3a. 同じ Taker 法人の在庫枠を合計で超える 2 件の売り(`concurrent-facility`)

Taker の正本上の在庫は USD/JPY 428 単位(Gateway が DeFMI の最終請求権から復元した値)、法人モジュールの枠は 300。
7 ノード停止中に売り 200 を 2 件続けて投入した。

- A(seq 16、request `b4676615…`)は署名され `queued` になった。
- B は署名される前に `the previous Taker reservation is still active` で拒否された(`02-rfq-b.json`)。
  Taker モジュールの境界は未決の予約を 1 件しか持たない。outbox に B の項目は無く、DeFMI に B の hold も無い
  (`03-both-submitted.json` の項目は seq 16 のみ)。
- ノード復帰後に A は 7 ノードで実行され、2 社が適格だったが限界価格内の値が付かず、hold `cdcd3079…` は
  `released`(高さ 359、遷移 `87bfe7eb…`)で決済されなかった(`04-a-finalized.json`)。
- A が最終状態になった後に同じ売り 200 を再投入すると受理され(seq 17)、同様に no-fill で解放された(`06-rfq-c.json`)。

つまり同一 Taker からの 2 件目は、法人モジュールの枠検査(`queued RFQs exceed the corporate cash or inventory limit`)より
手前の「未決予約は 1 件」で止まる。二重予約は署名の前に無くなる。合計枠の検査そのものは
`rust/qomm-demo/src/participant_node.rs` の `enqueue_outbox` が outbox ファイルロックの下で行い、正本側の検査は
`rust/defmi/tests/facility.rs` の同時 RFQ 試験が gate で走る。

### 3b. 同じ Maker pool を合計で超える 2 件の買い(`concurrent-pool`, `pool-sum`)

最初の試み(`concurrent-pool`)は投入自体が `Taker has 968198 cash units available but the signed limit needs 2386050`
で拒否された。Taker の正本現金は 9,681.98(現金単位 968,198)で、150 単位 × 159.07 の買いは払えない。
outbox に項目は作られず、DeFMI にも何も起きていない(`03-rfq-a.json`, `04-rfq-b.json`)。

そのため数量を組み直した `pool-sum` を追加した: Maker 3 だけを有効にし、pool を 30 単位、在庫スキューを
上限 120 に固定(約定後の自動スキュー更新で方針が変わると新しい pool が sequence 0 で登録されてしまうため)、
買い 20 を 2 件。1 件目が決済した後の残余は 10 なので、2 件目は残余を超える。1 件目が予約中に出した 2 件目は
3a と同じ理由で署名前に拒否される。1 件目の最終状態の後に出した 2 件目は、pool の残余を超える配分を
DvP の残余範囲証明が拒み、決済も pool sequence の前進も起きないことを判定する。この試験の記録は
`pool-sum.*` として同じディレクトリに置く。

最初の実行(16:17–16:33 UTC、`attempt1-pool-sum/`)では 1 件目が約定しなかった。在庫スキューを +120 に固定したまま
スキュー係数 1・深さ係数 3 を残したことで Maker 3 の売り気配が Taker の既定の買い限界(参照値 + 1%)の外へ出たためで、
hold `fc1aa2df…` は no-fill として高さ 456 で解放された。その際、生きているラウンドは
`DeFMI accepted a different Taker no-fill refund` で止まり(Gateway が用意した返金遷移より先に Taker モジュール自身の
解放を正本が受理した)、法人側の正本照合が hold を `released` で閉じた。正本は一貫している(hold 1 つ、解放 1 回、決済なし)が、
Gateway は再実行に備えて保持した Taker 予約を解かず、2 回目の実行(16:35 UTC)は 1 件目の署名前に
`the previous Taker reservation is still active` で拒否された。これが「使ったもの」に書いた 2 点目のゲートウェイ修正の
動機で、修正を入れたイメージ(`sha256:414bee5f…`)で Gateway だけを作り直してから 3 回目を実行した。

3 回目(16:57–17:13 UTC、`pool-sum.*`)。Maker 3 の方針は `active 1, asset 0, maxqty 30, inv 120, invcoef 0, slope 0`
(`01-policy-maker3-on.json`)、他 3 社は `active 0`。ラウンド開始時に Maker 3 の pool 対が新しい id で再登録され、
7 ノードの世代は 86 → 88(ノード 0 は 87 → 89。登録開示値の再配布 2 回、DeFMI の sequence は動かない)。

- A(seq 20、request `937e8a3c…`、digest `cc8b0224…`、hold `acb27efd…`)は予約(`04-a-hold-active.json`: `dispatching`,
  hold `active`)の後に約定し、DeFMI の standing pool `e72367c4…`(Maker 3、買い方向)から配分された。7 ノードの
  `/v1/rounds` 受領証(round `57951ba5…`、実行世代 1)から再計算した残余 note の id は pool の `currentPoolNoteID`
  `69c47e2d…` に一致し、pool は sequence 0 → 1、7 つの部分コミットメントの和は残余 note のコミットメントに一致する。
  各ノードは世代 88(ノード 0 は 89)で実行し、決済後のコミットで 89(ノード 0 は 90)になった(`06-a-finalized.json`,
  `07-after-a.json`)。Gateway の Taker 投影は在庫 429 → 449、現金 952,473 → 637,673(20 単位 × 15,740)。
- A のラウンド中に出した B(`05-rfq-b-concurrent.json`)は、ラウンドが走っている間 Gateway が新しい席に画面を返さないため
  30 秒の期限内に投入できなかった(`no gateway view before the deadline`)。署名も hold も無く、
  A の後の outbox 項目は seq 20 だけ(`07-after-a.json`)。
- A の最終状態の後に出した B(`08-rfq-b.json`、買い 20)は、Gateway が Maker 3 の予約を pool の残余(10)に合わせて
  更新している一方で standing mandate は 30 のままなので、エンジンが
  `Maker 3 policy or reserve changed without a fresh standing mandate` で hold を作る前に fail closed した
  (`abort_code: engine`、ラウンド 2、`reserve_released`)。outbox に項目は作られず(`next_sequence` は 21 のまま)、
  pool `e72367c4…` は sequence 1 のまま、7 ノードの世代も 89(ノード 0 は 90)のまま、Taker 投影も変わらない
  (`11-after-b.json`, `99-after.json`)。420 秒後も同じ(`10-b-later.json`)。

つまり合計 40 の買いに対して pool 30 から決済されたのは A の 20 だけで、B は残余を超える配分に到達する前に
Gateway の予約鏡(mandate と予約の不一致)で止まった。DvP の残余範囲証明はその奥にある二重目の守りで、この試験では
手前の守りが先に効いた。判定は `pool-sum.judge.json`(全項目 pass)、識別子と残高は `acceptance.json` の
`scenarios.pool-sum` にある。

### 3c. 同じ Maker pool に対する 2 件の署名済み要求を両方とも耐久受理させる(`pool-race`)

3b の 2 件目は Gateway の予約鏡(mandate と予約の不一致)で止まり、法人モジュールにも DeFMI にも届かなかった。
そこで、2 件目が Gateway の部屋の規則に触れずに法人モジュールへ耐久受理される経路を使う。Gateway は
「未決の Taker 予約は 1 件」をプロセス内の投影として持つだけで、法人モジュール(`participant_node.rs` の
`enqueue_outbox`)は署名検証と法人枠の合計検査を通る要求を何件でも耐久受理する。委員会を `docker pause` で
到達不能にしたまま A を署名して `queued` にし、Gateway を再起動して投影を空にし、席の固定と同じ方針集合を
再適用してから(席・固定・方針は Gateway のメモリにあり、再起動で bot に戻る)B を署名して `queued` にする。
復帰後、再送ループが保存済みの方針・予約スナップショットから A、B の順に実行する。要求は売り(Taker が在庫を渡し、
Maker の現金 pool が上限)で、pool は 3 単位、各 2 単位: 長く走っているデモの Taker の正本現金 note はそれまでの買いで
8,073 現金単位まで減っていて買いは Taker 自身の枠で署名前に拒否されるためで、在庫の note は数百単位ある。

結果(8 回目、01:33–01:52 UTC 9/3、`pool-race.*`; pool 9 単位・売り 5 単位 × 2; 1〜7 回目は `attempt*-pool-race/`。
7 回目は pool 7 単位に売り 2 単位で合計が pool を超えておらず前提を満たさない):

- A(seq 34)と B(seq 35)は別 request id・別 digest で両方 `queued` になった(`07-both-queued.json`)。
- 復帰後 A は決済した(高さ 700)。Gateway の日誌にある A の配分遷移(`11b-journal-1.json`)は pool `ce968628…` を
  sequence 0 で名指し、B の後に正本へ問い合わせた状態(`11c-pool-a.json`)でもその pool は sequence 1、現在 note は
  配分の残余 note `db79aac6…` のままである。
- B は再実行で **DeFMI に拒否された**: `Avalanche consensus rejected transaction …: admission batch is not the next venue
  sequence`(`pool-race.gateway.log`)。B の署名済み envelope は署名時の venue 入場連番を持ち、A の入場で連番が進んだ後は
  正本の入場連番の compare-and-swap に合わない。Gateway はこれを受けて B を hold を作る前に終端した
  (`aborted_before_reserve`、`10-b-finalized.json`, `11-after-b.json`)。決済も hold も無く、B の配分遷移は日誌に存在しない。

6 回目(`attempt6-pool-race/`、pool 3 単位)では B の 1 回目の実行が MPC の照合まで進んだ後に Gateway の予約鏡
(`pre-trade reserves do not cover the matched sell`)で止まり、次のラウンドで Gateway が Maker 3 の pool を残りの正本 note から
新しい pool id で再エスクローして B はその新しい pool から決済した(高さ 577)。どちらの回でも、A が消費した pool から
2 件目が配分されることは無かった。誠実な Gateway は pool を超える配分を DeFMI へ提示しないので、pool note 自体の
compare-and-swap には正当な経路では到達しない。それを実機で確かめるのが 3d。判定(`pool-race.judge.json`)は
「A の配分が名指す pool の sequence と現在 note が B の後も変わらないこと」「B が A の pool から決済していないこと(fail closed、
または再エスクローされた別 pool からの決済)」「Gateway の記録があること」を要求する。

### 3d. 正本の pool 守りそのものを、受理済みの配分遷移の再提示で確かめる(`pool-replay`)

Gateway は `QOMM_DEFMI_JOURNAL_DIR` に、DeFMI へ発行した遷移(method と params)をそのまま日誌に書く
(`rust/defmi/src/avalanche.rs` の `journal_issued_transition`。内容は全て正本が受け取るものなので秘密はない)。
1 件の売りを通常経路で決済させ、その配分遷移 `issueStandingNotePoolAllocation` を日誌から取り出して
(`06-journaled-allocation.json`)、受入 CLI の `defmi-rpc` でそのまま再提示する(`07-replay.json`)。遷移・証明・委員会署名は
受理済みのもので、改変も迂回もない。正本は、pool の現在 note が遷移の名指す親 note ではなくなっているため拒否しなければ
ならない(pool note の compare-and-swap)。判定は、日誌の配分が決済の引き当てと一致すること(pool id、期待 sequence + 1、
残余 note = 現在 note)、再提示が拒否されたこと、拒否後に pool sequence・現在 note・状態根・outbox・ノード世代が
動いていないことを要求する。

結果(5 回目、00:00–00:16 UTC 9/3、`pool-replay.*`; pool 6 単位、売り 2 単位): seq 31 が pool `22479b1e…` から決済(高さ 650、
sequence 0 → 1)。

- 同一バイト列の再提示(`07-replay.json`): 正本は同じトランザクション id を返し、`txStatus` は元の高さ 650 の `accepted` を
  示す(重複排除)。状態根・pool・outbox・ノード世代は動かない。
- 同じ配分を現在の状態根の下で再提示(`07b-replay-fresh-root.json`): 新しいトランザクションとして受け付けられ、ブロック処理で
  `k-of-n DeFMI approval is invalid` として**拒否**された。k-of-n 承認は期待状態根を束縛しているため、根を差し替えた遷移は
  承認検査で落ちる。状態は動かない(`08-after-replay.json`)。

したがって pool note の compare-and-swap(`rust/defmi-avalanche-vm/src/execution.rs` の
`standing allocation is stale or outside its Maker mandate`)は、正当な承認者集合が stale な配分に改めて署名した場合にだけ
到達する最後の守りで、誠実な Gateway・誠実な承認者の経路では手前の層(Gateway の鏡、トランザクション id の重複排除、
期待状態根を束縛する k-of-n 承認)が先に止める。この compare-and-swap 自体を直接叩く単体試験は workspace に無い
(残課題として記録)。

## 4. ラウンド途中でノードを止める

いずれも `wait --until hold-active` で DeFMI の hold が `active` になった直後(execute 開始後)にノードを止めた。
閾値 T=2、n=7。

| シナリオ | 止めたノード | seq / request / hold | 停止中 | 復帰後 |
|---|---|---|---|---|
| `abort-1` | mpc-6 (1) | 13 / `dc494861…` / `cf991faa…` | hold `active`、決済なし、6 ノード健全・1 ノード不達 | 決済 1 回、高さ 235、遷移 `fd15fcc4…` |
| `abort-2` | mpc-5, mpc-6 (2 = T) | 14 / `8bddaaaa…` / `86bfaa8e…` | 同上、5 健全・2 不達 | 決済 1 回、高さ 267、遷移 `7e87f557…` |
| `abort-3` | mpc-4, mpc-5, mpc-6 (3 > T) | 15 / `cd8cff71…` / `fbdc6513…` | 同上、4 健全・3 不達 | 決済 1 回、高さ 299、遷移 `d9da7937…` |

生きているラウンドは 3 件とも `the signed RFQ remains reserved in DeFMI and is awaiting canonical reconciliation`
で止まり(`01-rfq.json`、abort-1 は 92.7 秒後、abort-3 は接続タイムアウトで 212 秒後)、部分的な委員会で決済する
経路は無い。停止中のスナップショット(`03-after-abort.json`, 15 秒後の `04-still-reserved.json`)で、hold は `active`、
outbox は `dispatching`、pool の sequence は動かず、7 ノードの世代は揃ったまま(止めたノードは `/health` 不達)。
ノードを起こすと再送ループが同じ seq・同じ hold で再実行し、一度だけ決済した。7 ノードの世代は同じ差分で進む。

閾値内と閾値超で挙動が同じである理由: MP-SPDZ の malicious-Shamir は n=7, T=2 で「不正を検出して止まる」
までであり、欠けた参加者抜きで続行する(訂正する)には n ≥ 4T+1 = 9 が要る(`DEMO.md` の設計点)。
したがって 1 台欠けても安全に「再開」できるのは、欠けたノードが戻ってから同一要求を再実行する形であり、
これは 3 件とも同じ経路である。閾値超では 4 ノードしか残らず、次数 2T の開示に必要な 2T+1 = 5 に届かないので、
復元も決済も起こせない(fail closed)。復旧後の決済が一度だけであることは、同じ hold の `consumed` が 1 回、
pool の sequence 前進が 1 回、outbox の受領証が 1 つであることで判定した。

なお各ラウンドの開始時に 8 つの standing pool が新しい id で登録し直され(判定では `registered_at_zero` として区別)、
7 ノードの世代は rebind と決済後 commit の分だけ揃って進む。

## 5. 7 ノード・Gateway・DeFMI をまたぐ再起動と Rust の gate

`restart-all`: 7 ノード(1788336274)、DeFMI ネットワーク(1788336289)、Gateway(1788336371)の順に再起動した。
再起動直後のスナップショットで 7 ノードは健全、世代は再起動前と同じ、pool の sequence も同じ。
その後の買い 1 単位(seq 18、request `54f2fb27…`、hold `526457e6…`)は、Maker 3 の以前からの pool `2b0c53a8…`
(sequence 1)を 7 ノードが受理済み実行から `catch_up` して開き、sequence 2 へ進めて決済した(高さ 437、遷移 `1434ec55…`)。
常駐 Maker 状態が 3 サービスの再起動をまたいで正本と再同期できることの実証である。

Rust の gate(`make rust-test`: fmt --check、clippy `-D warnings`、release 全試験)は host-a の
`qomm-test` イメージで走らせる。同イメージに rustfmt が無かったため `demo-network/Dockerfile.test` から作り直した。
共有 checkout には他セッションの作業中ファイル(`rust/qomm-sim/tests/*matches_python*`、
`rust/aethel-core/tests/protocol.rs`)があり、それらの状態で workspace 全体の合否が変わる。本作業で変えた
`qomm-demo` は fmt・clippy `-D warnings` を通した。

## 6. Rust gate を全て緑にするための修正(2026-09-03)

共有 checkout 相当の木(この作業のファイルを重ねたもの)で `cargo fmt --all -- --check`、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`cargo test --workspace --all-targets --all-features --release --no-fail-fast`、`cargo build --release --workspace` を
host-a(softbank)で走らせ、赤かった項目を次のように直した。ログは `artifacts/live_acceptance/2026-09-02/gates/`。

- `rust/defmi/tests/facility.rs`: 引数 8 個の試験補助関数 `certified_admission_population` を
  `AdmissionPopulation` 構造体で受ける形にした(clippy `too_many_arguments`)。振る舞いは同じ。
- `rust/qomm-mpc` の生成器契約(V7 → V8): 2026-09-01 の生成器の変更(DvP 証人が勝者 Maker の pool-before 開示値と
  その残余の範囲証明を持ち、Taker の買い側現金予約を価格×数量にする。常駐 Maker 状態の作業の一部で、
  「MPC の pool 残余が正本 parent note を保存する」ための変更)以後、27 件の全ファイル契約と 6 件のうち 1 件の
  program 契約、`dvp_handoff.rs` の期待行が古いままだった。生成器の変更は意図されたものと判断し
  (変更内容が常駐化タスクの目的そのもので、`tests/dvp_handoff.rs` はその変更を検証する試験として同時に更新されている)、
  契約を V8 として再発行した。V8 の値は host-a の生成器出力から取り、host-b の独立実行と一致することを確かめた。
  出所と差分の説明は各契約定数の直前のコメントに書いた。ハッシュだけを置き換えたのではなく、`dvp_handoff.rs` は
  新しい証人行(`dvp_maker_pool_before` / `dvp_maker_delivery` / `dvp_maker_pool_remainder` とその bit 分解)を要求する。
- `rust/zkfmi-measure/src/hosts.rs` の「出荷ファイルに実機名が無い」試験: この作業の文書と、他セッションの
  project-memory・受入記録・UI 監査 README・Makefile に実機名が入っていた。文書側は公開ラベル(`host-a`/`host-b`)へ
  置換し、Makefile の承認済み遠隔ホスト一覧は出荷しない `scripts/remote_hosts.txt`(`.gitignore` 済み、先頭が既定)
  へ移して `REMOTE_TEST_HOST` は環境変数からも受け取るようにした。どちらも無ければ `remote-test` は
  「重い試験は承認済み遠隔ホストでしか走らない」旨で拒否する。要件(遠隔実行の強制)は残したまま、
  実機名だけを設定境界の外へ出した。
- `rust/qomm-harness/src/bin/run_transport.rs` の時間依存試験 `every_relay_hop_costs_a_connection`: 各 slot を時計だけで
  閉じていたため負荷の高いホストでフレームが落ち、壁時計の中央値比較も負荷で逆転し得た。`run_session` は
  各 hop に送ったフレームが届いたことを観測してから slot を閉じるようにし(`Relay::pending_frames`)、hop の費用は
  時間でなく受け付けた接続数(`Relay::accepted`: 初段は client 数、以降の hop は閉じた slot ごとに 1)で数える。
  3 回連続で pass し、host-a・host-b の両方で同じ結果になる。
- `rust/qomm-harness/tests/fill_fold.rs` は `artifacts/fill_fold.json` を読む。gate 用の同期が `artifacts/` を除外していた
  ための失敗で、同期を直した(`artifacts/tapes/` だけ除外)。

最終 gate(host-a、2026-09-03): fmt pass、workspace clippy `-D warnings` 警告 0、195 test suite 全 pass、release build pass。


#### (iii) pool を超える remainder を正本 VM の実行に到達させる（`07c-pool-guard.json`）

(i)(ii) は k-of-n approval と root の守りで止まる。**pool 固有の守り**そのものを踏ませるため、
`qomm-live-acceptance pool-guard-probe` を追加した。Gateway が日誌に残した配分遷移（実際に受理されたもの）を
そのまま使い、`expectedBeforeRoot` を**その時点の生の global root** に置き、Maker の remainder note の
value commitment だけを `remainder + escrow` に膨らませて（child + remainder が parent を超える）、
第一級の `defmivm.issueStandingNotePoolAllocation` として DeFMI へ投入する。approval・証明ダイジェスト・
委員会署名は Gateway 自身のもので、偽造は無い。remainder note の note id は canonical な `NoteOutput::derived_id`
で再計算しているので note 自体は整合している。

正本 VM の `allocate_standing_note_pool` は、まず `allocation.body()` で親 commitment の保存
（`previous == escrow + remainder`）を検査し、**approval と root を見る前に**
`standing allocation does not conserve its parent commitment` で拒否する。これは「pool が持つ以上を取る配分」に
対する standing pool 固有の守りであり、Gateway でも approval 層でもなく consensus の実行で fail closed する。
記録は `07c-pool-guard.json`（`expected_before_root_is_live: true`、`rejected: true`、
`rejected_by_pool_conservation_guard: true`、`state_root_unchanged: true`）。最終回の実測: pool `3331475a…` の受理済み配分を
膨らませた tx `RE34R2J3pDwo27EwYPtVpREVSdHnCMh1hvqdKCTgNB1NpsWAY` は mempool に受理された後、consensus 実行で
`standing allocation does not conserve its parent commitment` として拒否され、root は不変だった。

sequence / current-note の compare-and-swap（`standing allocation is stale or outside its Maker mandate`）は
同じ関数で approval 検査の**直後**にあり、conserving な配分に現在 root への有効な approval を付け直さなければ
到達できない。(iii) はその手前の同じ standing-pool 配分本体で、pool を超える配分を正本が拒むことを実機で示した。

#### (iv) sequence / current-note の compare-and-swap そのものを踏ませる（`07d-pool-cas.json`）

(iii) は保存則の守りで、approval と root の前に止まる。ご指摘の **sequence / current-note の
compare-and-swap 本体**（`standing allocation is stale or outside its Maker mandate`）は、正本 VM の
`allocate_standing_note_pool` で approval 検査の**直後・委員会署名の前**にある。ここに到達するには、
保存則を満たす配分を、現在 root への**有効な k-of-n approval** を付けて出すしかない。`qomm-live-acceptance
pool-cas-probe` はそれを構成する。

- Gateway 日誌の受理済み配分を土台に、fresh な operation id / hold id を振り、escrow note をその hold へ
  ロックし直して note id を再計算する（保存則は元の commitment のまま維持）。
- reserve nullifier・typed reserve digest・asset link digest は自由な値ではなく、配分と transition
  statement から**導出**される（`standing_pool_reservation_metadata`）。fresh な hold で transition
  statement が変わるため、導出される nullifier は自動的に未使用の新しい値になり、これで already-used の
  守りを越えられる。導出値を authorization に入れ直す。
- 開発委員会（`development_committee`、7 鍵は `sha256("key:i")` 由来、3-of-7、epoch 1、domain=chain id）の
  **node-1 / node-3 / node-5**（bootstrap の 0/1/2 とは別の 3 ノード）で、現在の global root に対して approval を
  署名する。VM が検証するのと同じ委員会で、署名・承認検証は一切無効化していない。
- expected_pool_sequence / previous_pool_note_id は、fill が既に進めた**過去の値**のまま提示する。

同じ土台で 3 つの transition を `defmivm.issueStandingNotePoolAllocation` へ出す:

| ケース | 変える点 | 期待する拒否理由 | 意味 |
|---|---|---|---|
| cas | 現在 root への有効 approval・stale pointer | `standing allocation is stale or outside its Maker mandate` | pool 固有の sequence/current-note CAS 本体 |
| bad-approval | 署名 1 バイトを反転 | `k-of-n DeFMI approval is invalid` | approval ゲートは CAS より前。cas の approval が本当に検証されたことの対照 |
| stale-root | fill 前の root で有効署名 | `transaction was built against a stale state root` | root ゲートは CAS より前。cas が生の root を使ったことの対照 |

VM の検査順が (approval → root → already-used → **CAS** → 委員会署名) である以上、cas ケースが CAS の理由で
落ちること自体が、approval が検証され root が現在値だったことの証明になる。bad-approval と stale-root は
より前のゲートで別の理由で落ち、cas との唯一の差分がそれぞれ「署名の有効性」「root の新旧」であることを示す。
記録（`07d-pool-cas.json`）には各ケースの tx id と `final_status.reason`、`pointer_is_stale_vs_canonical`、
`rejected_by_pool_sequence_current_note_cas`、`state_root_unchanged`、`pool_sequence_unchanged`、
`pool_current_note_unchanged` を残す。CAS 拒否の後、hold も settlement も無く、pool の sequence・現在 note・
state root はいずれも動かない。「正当な配分が通る対照」は同シナリオの実 fill（`05-finalized.json`、pool を
0→1 へ進めて決済）である。
