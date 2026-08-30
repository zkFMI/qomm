/* QOMM demo page.  No dependency.  The Rust room pushes a view V to each seat
   over one WebSocket and this file draws V: a live network diagram keyed on
   V.phase, the seat's own balances, a chat that turns plain sentences into
   the same request/policy messages the form sends, and the seat's panel.
   Nothing here knows more than V says: the server projects the round onto
   each connection and a projection can only delete. */
'use strict';

/* ========================================================================= */
/*  Strings                                                                   */
/* ========================================================================= */
const S = {
ja: {
  title:"QOMM", subtitle:"分割したまま計算して最良気配を選ぶ取引照合",
  engineSim:"Rust試作", engineSimShort:"Rust試作", engineSimNote:"分割計算は実装どおり。照合の比較は平文",
  engineMpc:"MP-SPDZ実行（価格照合）", engineMpcShort:"MPC価格照合", engineMpcNote:"価格照合まで秘密計算で実行。決済表示は説明用台帳",
  leave:"離席", langOther:"EN",
  connecting:"サーバーに接続しています…",
  reconnecting:"接続が切れました。再接続しています…",
  lobbyTitle:"参加する席を選ぶ",
  lobbyWhy:"席ごとに見える情報が異なります。それがこの仕組みの核心です。空席は自動で動きます。",
  yourName:"表示名（任意）", watch:"全体表示で観る（デモ専用）",
  theSeats:"空席は自動動作、名前が出ている席は人が操作中",
  whatIsThis:"このデモについて",
  publicTitle:"公開情報", publicWhy:"ラウンド終了後に全員が確認できる情報です。",
  historyTitle:"最近のラウンド",
  seatsTitle:"参加者", seatsWhy:"各席の動作状態です。",
  noticesTitle:"通知", footer:"QOMM demo — ",
  taken:"使用中", auto:"自動", manual:"操作中",
  seatTaker:"注文者", seatMaker:"値付け", seatNode:"計算ノード", seatObserver:"全体表示",
  // phases
  phaseDeal:"分割して配布", phaseCheck:"入力の検査", phaseReduce:"価格計算と照合",
  phaseOpen:"結果を読めない形で公開", phaseSettle:"結果検査とデモ決済", phaseIdle:"待機中", phaseDone:"完了",
  stopped:"停止", silent:"不参加", opened:"暗号化された結果", everyoneSees:"全員に公開",
  idleManual:"注文者の送信を待っています。",
  idleAuto:"次のラウンドを待っています。",
  noRoundYet:"まだラウンドがありません。",
  ph_deal:(f)=>`注文と価格方針を${f.values}個の値に分け、${f.nodes}台の計算ノードへ1つずつ配布しています。`,
  ph_check:(f)=>`配布された${f.values}×${f.nodes}件の値が配布記録と一致するか検査しています。`,
  ph_check_skipped:"入力の検査は無効です。配られたのと違う値を使うノードを誰も止められません。",
  ph_check_rejected:(f)=>`${nodeList(f.rejected)}の入力が配布記録と一致しません。計算前に除外し、停止しました。`,
  ph_reduce:(f)=>`分割したまま${f.products}回の掛け算を実行し、開いた値を検査しています。`,
  ph_reduce_mpc:(f)=>`MP-SPDZ が回路を実行しました（${f.rounds}ラウンド・${f.mb} MB）。`,
  ph_reduce_corrected:(f)=>` ${f.corrections}回を訂正し、${nodeList(f.named)}を特定しました。`,
  ph_open:"結果に注文者だけが外せる鍵を掛けて、全員へ開示しています。",
  ph_settle_settled:"結果検査を通過。説明用台帳が予約済みの在庫と資金を同時に更新しました。",
  ph_settle_released:(f)=>`予約を解放しました。${f.reason}`,
  ph_settle_cover:"ダミー通信のため、台帳は変更しません。",
  ph_done:(f)=>`ラウンド #${f.number} 完了。`,
  computeMs:(ms)=>`計算 ${ms} ms`,
  // graph
  graphAria:"取引の流れを示すネットワーク図",
  gTaker:"注文者", gMaker:"値付け", gNode:"ノード", gMatcher:"照合",
  gZkpi:"結果検査（説明用）", gLedger:"決済（説明用台帳）", you:"あなた",
  productBoundary:"この画面は、DeFMI/Avalanche専用VMで実装したzkPI検証と決済の流れを説明します。画面の台帳はRustメモリ上のモデルで、実L1へは送信しません。",
  legendNoCustody:"計算ノードは在庫も資金も保有しません。届くのは分割された値だけです。",
  legendLines:"動く線: いま流れている情報 ・ 淡い線: この席には見えない経路",
  eShares:"分割した値", eMyOrder:"あなたの注文を分割", eMyPolicy:"あなたの方針を分割",
  eCompute:"分割したまま計算", eOpen:"鍵付きの結果", eVerify:"検証", eLedger:"台帳を更新",
  eSettleTaker:"決済: 在庫と資金", eRelease:"予約を解放", eSettleMaker:"決済: 約定分",
  eNoChange:"変更なし",
  nIdle:"待機", nReceiving:"受取中", nChecking:"検査中", nComputing:"計算中",
  nDone:"完了", nSilent:"不参加", nRejected:"除外", nNamed:"不正を訂正", nStopped:"停止",
  mCompare:"全社の気配を比較", mPick:"最良気配を選定", mNone:"該当なし",
  zPass:"参照値と一致", zFail:"参照値と不一致", zDecoded:"開示値の検査を通過",
  zStopped:"不合格・停止", zWait:"待機",
  lSettled:"決済完了", lReleased:"予約を解放", lCover:"ダミー・変更なし", lWait:"待機",
  orderHidden:"注文は非公開", policyHidden:"方針は非公開",
  activeShort:"稼働", inactiveShort:"停止", maxShort:"最大", winner:"約定相手",
  // portfolio
  pfCash:"決済資金", pfAvailable:"利用可能", pfReserved:"予約中", pfTotal:"合計",
  pfDeltaReserve:"予約", pfDeltaSettle:"決済", pfDeltaRelease:"解放", pfDeltaUpdate:"予約枠更新",
  pfNodeNote:"このノードは在庫も資金も保有しません。届くのは分割された値だけで、元の注文や価格は復元できません。",
  pfObserverTitle:"全参加者の在庫と資金（デモ専用の全体表示）",
  pfMine:"あなたの在庫と資金",
  // chat
  chatTitle:"チャットで指示",
  chatWhyTaker:"日本語の指示を、下の操作パネルと同じ「注文の設定」と「送信」に変換します。送る前に内容を確認できます。",
  chatWhyMaker:"日本語の指示を、下の操作パネルと同じ「価格方針の更新」に変換します。最大数量や稼働を変えると予約枠も変わります。送る前に内容を確認できます。",
  chatPlaceholder:"例: USD/JPY を100単位 買いたい",
  chatPlaceholderTaker:"例: USD/JPY を100単位 買いたい",
  chatPlaceholderMaker:"例: 最大数量を300にして",
  chatSend:"送信", chatConfirm:"この内容で送信しますか？", chatYes:"送信する", chatNo:"やめる",
  chatRaw:"実際に送るデータ", chatTech:"技術詳細",
  chatApplied:"設定に反映しました。", chatSubmitted:"注文を送信しました。",
  chatSent:"送信しました。", chatCancelled:"取り消しました。",
  chatUnknown:"指示を解釈できませんでした。例: ",
  chatBusy:"ラウンド進行中は送信できません。終わるまでお待ちください。",
  chatRefused:(r)=>`サーバーが受け付けませんでした: ${r}`,
  chatWelcomeTaker:"注文の設定を日本語で指示できます。例:「USD/JPY を100単位 買いたい」「上限を158.5にして」「この内容で送信」",
  chatWelcomeMaker:"価格方針を日本語で指示できます。例:「スプレッドを30に」「最大数量を300に」「一時停止」「対象を EUR/USD に」",
  chatSubmitDesc:"送信する（予約してラウンドを開始）",
  chatReserveCash:(f)=>`送信時に予約する資金: ${f.amount}（${f.qty} × ${f.limit}）`,
  chatReserveInv:(f)=>`送信時に予約する在庫: ${f.qty} 単位（${f.asset}）`,
  chatReserveMaker:(f)=>`予約枠: 売却用在庫 ${f.inventory} 単位・買付用資金 ${f.cash}（現在: ${f.oldInventory} 単位・${f.oldCash}）`,
  chatClamped:(f)=>`${f.name}は ${f.lo}〜${f.hi} の範囲に丸めました。`,
  chatBothSides:"「買い」と「売り」の両方が含まれています。どちらか一方にしてください。",
  hintsTaker:["USD/JPY を100単位 買いたい","上限を 158.5 にして","ダミー通信に切り替え","実注文に戻して","この内容で送信"],
  hintsMaker:["スプレッドを30に","最大数量を300に","一時停止","再開","対象を EUR/USD に"],
  // taker
  takerTitle:"注文の設定",
  takerWhy:"送信すると資金または在庫を予約し、注文を分割して各ノードへ送ります。",
  asset:"銘柄", side:"売買", buy:"買い", sell:"売り", qty:"数量",
  limitPrice:"価格条件", limitPriceBuy:"支払う最高価格", limitPriceSell:"受け取る最低価格",
  limitBuy:"この価格以下なら自動決済", limitSell:"この価格以上なら自動決済",
  kind:"種別", real:"実注文", cover:"ダミー通信",
  coverWhy:"ダミー通信は実注文と同じ計算・通信量・時間で走ります。外からは区別できません。",
  automaticSettle:"約定すると、説明用台帳が予約済みの在庫と資金を同時に更新します。追加の署名は要りません。実DeFMI/Avalanche経路は別の受入試験で動作します。",
  submit:"送信する", waiting:"実行中…", reserveOnSend:"送信時に予約",
  openedTitle:"開示された結果",
  openedWhy:"全員がこの値を見ています。読めるのは鍵を持つあなただけです。",
  minusMask:"あなたの鍵で読む",
  yourPrice:"約定価格", winnerIs:"約定相手",
  noMaker:"条件に合う相手がいませんでした",
  eligible:"条件を満たした相手数",
  coverRound:"このラウンドはダミー通信でした。価格は使われません。",
  settlementTitle:"照合と決済", noSettlement:"まだ決済結果はありません。",
  status:"状態", settled:"決済完了", released:"予約を解放", coverStatus:"ダミー（残高変更なし）",
  settlementCash:"受渡資金", stateRoot:"台帳ハッシュ", automatic:"追加署名なしで決済",
  sr_cover:"ダミー通信のため台帳は変更していません。",
  sr_mpc_aborted:"計算が安全に停止したため予約を解放しました。",
  sr_no_maker:"条件を満たす相手がいないため予約を解放しました。",
  sr_price_limit:"約定価格が上限の外だったため予約を解放しました。",
  sr_automatic_dvp:"予約済みの在庫と資金を台帳が同時に更新しました。",
  price:"価格",
  // maker
  makerTitle:"価格方針",
  makerWhy:"注文は見えません。方針と最大数量を先に登録し、見えない注文に対して評価されます。",
  ask_level:"基準価格への上乗せ", spread:"売値と買値の差", slope:"数量が増えた時の調整", invcoef:"在庫に応じた調整の強さ",
  inv:"現在の在庫調整", maxqty:"最大数量", active:"稼働", assetLabel:"対象銘柄",
  invWhy:"＋で両方の気配を上げ（買い戻したい）、−で下げます（売りたい）。",
  reserveTitle:"予約枠（方針から自動計算）",
  reserveWhy:"最大数量ぶんの売却用在庫と、買付用資金を方針の登録時に確保します。",
  inventoryReserve:"売却用在庫", cashReserve:"買付用資金",
  fillTitle:"約定通知", noFill:"今回あなたへの通知はありません。",
  noFillWhy:"落札できなかったのか、別の銘柄だったのかは分かりません。",
  yourFill:"あなたが約定しました",
  tryTitle:"価格試算（手元のみ）",
  tryWhy:"この計算はブラウザ内だけで行われ、サーバーには送られません。",
  tryQty:"数量", ask:"売り気配", bid:"買い気配", on:"稼働", off:"停止",
  // node
  nodeTitle:"保有データ",
  nodeWhy:"注文と方針を分割した値です。このノード単体では元の値を復元できません。",
  noCustody:"計算ノードは在庫や資金を保有しません。分割された値で計算し、検証結果と台帳ハッシュだけを確認します。",
  behaviourTitle:"ノードの動作設定",
  behaviourWhy:"選んだ動作はあなた以外には見えません。結果だけが公開されます。",
  verdictTitle:"今回の判定",
  namedYou:"あなたの不正が検出されました", namedNobody:"不正は検出されませんでした",
  corrected:"訂正した不正値", capacityProduct:"価格計算で訂正できる不正ノード数", capacityOpen:"結果開示で訂正できる不正ノード数",
  rejectedYou:"あなたの入力が配布記録と一致しませんでした。計算前に除外されています。",
  refused:"除外",
  b_honest:"正しく計算する", b_honest_d:"何も起きません。正常動作です。",
  b_lie_product:"価格計算中に不正な値を送る",
  b_lie_product_d:"不正は検出・訂正され、計算は続きます。訂正能力の上限まで。",
  b_lie_open:"結果公開時に不正な値を送る", b_lie_open_d:"検出・訂正されます。",
  b_lie_input:"配られたのと違う入力を使う",
  b_lie_input_d:"入力の検査が有効なら事前に検出されます。無効なら誰も気づかず結果が狂います。",
  b_dropout:"入力後に無応答になる", b_dropout_d:"訂正できる不正の数が減ります。",
  b_offline:"最初から参加しない", b_offline_d:"入力が欠けるため値そのものが失われます。",
  inertTitle:"外部エンジンが実行中", inertWhy:"この動作は外部エンジンには反映されません。",
  verifiedYes:"参照値と一致", verifiedNo:"参照値と不一致",
  protocolMs:"計算時間", engRounds:"通信ラウンド数", engMb:"通信量 (MB)", compiledOnce:"初回コンパイル",
  // observer
  observerTitle:"全体表示（デモ専用）",
  observerWhy:"実運用にこの画面はありません。デモのための全情報表示です。",
  allQuotes:"全参加者の気配", maker:"値付け", node:"ノード",
  reason:"理由", request:"注文", behaviours:"ノードの動作", settings:"設定",
  roundEvery:"自動ラウンド間隔（秒）", stepMs:"段階ごとの表示時間（ms）",
  autoRounds:"自動ラウンド", inputCheck:"入力の検査",
  inputCheckWhy:"無効にすると、配られたのと違う入力を使うノードを誰も止められなくなります。",
  runNow:"いま1回実行",
  // refusals from the server
  r_busy:"ラウンド進行中のため受け付けられません。",
  r_taker_cash:(f)=>`資金が足りません。利用可能 ${f.have} に対し、上限価格での予約に ${f.need} 必要です。`,
  r_taker_inv:(f)=>`在庫が足りません。利用可能 ${f.have} 単位に対し、注文は ${f.need} 単位です。`,
  r_maker_reserve:(f)=>`予約枠を確保できません。在庫 ${f.units} 単位と資金 ${f.cash} が必要です。`,
  r_taken:(f)=>`${f.seat} は ${f.who} が使用中です。`,
  // notices
  n_finished:(f)=>`#${f.number} 完了（${f.real?"実注文":"ダミー通信"}）`,
  n_corrected:(f)=>`#${f.number} ${f.reductions}回中${f.corrections}回を訂正。特定: ${nodeList(f.named)}`,
  n_stopped:(f)=>`#${f.number} 停止 — ` + abortWhy(f),
  n_refused:(f)=>`${nodeList(f.who)} を除外しました`,
  n_unchecked:(f)=>`${nodeList(f.who)} が配られたのと違う入力を使いました。検査が無効のため検出されていません。結果は不正確です`,
  n_claimed:(f)=>`${f.seat} に着席${f.label?"："+f.label:""}`,
  n_you_won:(f)=>`#${f.number} あなたが約定しました`,
  n_settled:(f)=>`#${f.number} 予約済みの在庫と資金を追加署名なしで決済しました`,
  n_reserve_released:(f)=>`#${f.number} ${t('sr_' + (f.reason||'no_maker'))}`,
  n_limit_released:(f)=>`#${f.number} 約定価格が上限の外だったため予約を解放しました`,
  a_beyond_capacity:(f)=>`応答${f.answered}台に対し、訂正できるのは${f.capacity}台まで`,
  a_absent:(f)=>`${nodeList(f.who)} が不参加。入力は全${f.n}台の値の和なので、1つ欠けると値が失われます`,
  a_commitment:(f)=>`${nodeList(f.who)} の入力が配布記録と一致しません`,
  a_too_few:(f)=>`${f.answered}台では復元に足りません（${f.needed}台必要）`,
  a_mismatch:(f)=>`結果が参照値と一致しません${f.detail?"："+f.detail:""}`,
  a_engine:(f)=>`外部エンジンが失敗しました${f.detail?"："+f.detail:""}`,
  explain:[
    ["注文者","注文を出す側です。注文は分割されて送られ、どの計算ノードにも全体は渡りません。約定価格を受け取れるのはこの席だけです。"],
    ["値付け参加者","価格方針と最大数量を先に登録します。注文は見えません。約定すると予約枠から自動で決済されます。"],
    ["計算ノード","分割された値だけを使って計算します。在庫や資金は保有しません。不正な動作を試すこともできます。"]
  ]
},
en: {
  title:"QOMM", subtitle:"best-quote matching computed on split values",
  engineSim:"Rust simulation", engineSimShort:"Rust test", engineSimNote:"real share layer; the comparison is in the clear",
  engineMpc:"MP-SPDZ (quote matching)", engineMpcShort:"MPC matching", engineMpcNote:"the quote circuit runs under secure computation; settlement shown here uses the demo ledger",
  leave:"leave", langOther:"JA",
  connecting:"connecting to the server…",
  reconnecting:"connection lost — reconnecting…",
  lobbyTitle:"Take a seat",
  lobbyWhy:"Each seat sees different information. That difference is the core argument. Empty seats run automatically.",
  yourName:"your name (optional)", watch:"watch everything (demo only)",
  theSeats:"empty seats run automatically; named ones have a person",
  whatIsThis:"About this demo",
  publicTitle:"Public information", publicWhy:"What everyone can see after a round.",
  historyTitle:"Recent rounds",
  seatsTitle:"Participants", seatsWhy:"Activity status of each seat.",
  noticesTitle:"Notices", footer:"QOMM demo — ",
  taken:"taken", auto:"auto", manual:"held",
  seatTaker:"taker", seatMaker:"maker", seatNode:"node", seatObserver:"observer",
  phaseDeal:"split & deal", phaseCheck:"check inputs", phaseReduce:"price & match",
  phaseOpen:"masked result", phaseSettle:"check & demo settlement", phaseIdle:"idle", phaseDone:"done",
  stopped:"stopped", silent:"absent", opened:"revealed", everyoneSees:"everyone sees this",
  idleManual:"Waiting for the taker to send.",
  idleAuto:"Waiting for the next round.",
  noRoundYet:"No round yet.",
  ph_deal:(f)=>`${f.values} values split into ${f.nodes} pieces, one per node.`,
  ph_check:(f)=>`Checking ${f.values} × ${f.nodes} pieces against the dealing records.`,
  ph_check_skipped:"Input check is off: nothing stops a node using a value it was not dealt.",
  ph_check_rejected:(f)=>`${nodeList(f.rejected)} stated a value the dealing record does not bind. Excluded and stopped.`,
  ph_reduce:(f)=>`${f.products} multiplications on split values; the openings are decoded and checked.`,
  ph_reduce_mpc:(f)=>`MP-SPDZ ran the circuit (${f.rounds} rounds, ${f.mb} MB).`,
  ph_reduce_corrected:(f)=>` ${f.corrections} corrected, named ${nodeList(f.named)}.`,
  ph_open:"The result is revealed under a key only the taker can remove.",
  ph_settle_settled:"Checked. The explanatory ledger moved both pre-reserved legs at once.",
  ph_settle_released:(f)=>`Reserve released. ${f.reason}`,
  ph_settle_cover:"Dummy traffic: the ledger is unchanged.",
  ph_done:(f)=>`Round #${f.number} done.`,
  computeMs:(ms)=>`compute ${ms} ms`,
  graphAria:"network diagram of the trade flow",
  gTaker:"Taker", gMaker:"Maker", gNode:"Node", gMatcher:"Match",
  gZkpi:"Result check (demo)", gLedger:"Settlement (demo ledger)", you:"you",
  productBoundary:"This screen explains the zkPI verification and settlement flow implemented by the DeFMI/Avalanche custom VM. Its ledger is an in-memory Rust model and does not submit to the live L1.",
  legendNoCustody:"Nodes hold no inventory or cash; only split values reach them.",
  legendLines:"moving line: information in flight · faint line: a path this seat cannot see",
  eShares:"split values", eMyOrder:"your order, split", eMyPolicy:"your policy, split",
  eCompute:"computed on split values", eOpen:"keyed result", eVerify:"verify", eLedger:"ledger update",
  eSettleTaker:"settle: inventory & cash", eRelease:"reserve released", eSettleMaker:"settle: fill",
  eNoChange:"no change",
  nIdle:"idle", nReceiving:"receiving", nChecking:"checking", nComputing:"computing",
  nDone:"done", nSilent:"absent", nRejected:"excluded", nNamed:"corrected", nStopped:"stopped",
  mCompare:"comparing quotes", mPick:"best quote picked", mNone:"no match",
  zPass:"matches reference", zFail:"does not match", zDecoded:"openings decoded",
  zStopped:"failed — stopped", zWait:"idle",
  lSettled:"settled", lReleased:"released", lCover:"dummy — unchanged", lWait:"idle",
  orderHidden:"order not visible", policyHidden:"policy not visible",
  activeShort:"on", inactiveShort:"off", maxShort:"max", winner:"winner",
  pfCash:"settlement cash", pfAvailable:"available", pfReserved:"reserved", pfTotal:"total",
  pfDeltaReserve:"reserve", pfDeltaSettle:"settle", pfDeltaRelease:"release", pfDeltaUpdate:"reserve update",
  pfNodeNote:"This node holds no inventory or cash. Only split values arrive; the order and price cannot be rebuilt from them.",
  pfObserverTitle:"Every participant's balances (demo-only view)",
  pfMine:"Your inventory and cash",
  chatTitle:"Chat control",
  chatWhyTaker:"Plain sentences become the same order settings and send command as the panel below. You confirm before anything is sent.",
  chatWhyMaker:"Plain sentences become the same policy updates as the panel below. Max size and on/off also change your reserve. You confirm before anything is sent.",
  chatPlaceholder:"e.g. buy 100 units of USD/JPY",
  chatPlaceholderTaker:"e.g. buy 100 units of USD/JPY",
  chatPlaceholderMaker:"e.g. set maximum size to 300",
  chatSend:"Send", chatConfirm:"Send this?", chatYes:"Send", chatNo:"Cancel",
  chatRaw:"message payload", chatTech:"Technical details",
  chatApplied:"Applied to the settings.", chatSubmitted:"Order submitted.",
  chatSent:"Sent.", chatCancelled:"Cancelled.",
  chatUnknown:"Could not interpret. Try: ",
  chatBusy:"A round is in progress; wait for it to finish.",
  chatRefused:(r)=>`Refused by the server: ${r}`,
  chatWelcomeTaker:"Describe the order. E.g. \"buy 100 units of USD/JPY\", \"limit 158.5\", \"send\".",
  chatWelcomeMaker:"Describe the policy. E.g. \"spread 30\", \"max size 300\", \"switch off\", \"market EUR/USD\".",
  chatSubmitDesc:"send (reserve and start a round)",
  chatReserveCash:(f)=>`cash reserved on send: ${f.amount} (${f.qty} × ${f.limit})`,
  chatReserveInv:(f)=>`inventory reserved on send: ${f.qty} units (${f.asset})`,
  chatReserveMaker:(f)=>`reserve: ${f.inventory} units to sell, ${f.cash} cash to buy (now ${f.oldInventory} units, ${f.oldCash})`,
  chatClamped:(f)=>`${f.name} clamped to ${f.lo}–${f.hi}.`,
  chatBothSides:"Both buy and sell were mentioned; pick one.",
  hintsTaker:["buy 100 units of USD/JPY","limit 158.5","dummy traffic","live order","send"],
  hintsMaker:["spread 30","max size 300","switch off","switch on","market EUR/USD"],
  takerTitle:"Order setup",
  takerWhy:"Sending reserves cash or inventory, then splits the order across the nodes.",
  asset:"market", side:"side", buy:"buy", sell:"sell", qty:"size",
  limitPrice:"price condition", limitPriceBuy:"maximum price to pay", limitPriceSell:"minimum price to receive",
  limitBuy:"auto-settles at or below", limitSell:"auto-settles at or above",
  kind:"kind", real:"live order", cover:"dummy traffic",
  coverWhy:"Dummy traffic uses the same circuit, bytes and time. Nothing outside can tell them apart.",
  automaticSettle:"On a match the explanatory ledger moves both pre-reserved legs at once. No further signature. The real DeFMI/Avalanche path runs in a separate acceptance test.",
  submit:"send", waiting:"running…", reserveOnSend:"reserved on send",
  openedTitle:"Revealed result",
  openedWhy:"Everyone sees this value. Only you hold the key that reads it.",
  minusMask:"read with your key",
  yourPrice:"matched price", winnerIs:"counterparty",
  noMaker:"no eligible counterparty",
  eligible:"eligible counterparties",
  coverRound:"that round was dummy traffic; the price is not used.",
  settlementTitle:"Match & settlement", noSettlement:"No settlement result yet.",
  status:"status", settled:"settled", released:"released", coverStatus:"dummy (no balance change)",
  settlementCash:"cash delivered", stateRoot:"ledger hash", automatic:"settled without a further signature",
  sr_cover:"Dummy traffic — the ledger is unchanged.",
  sr_mpc_aborted:"The computation stopped safely; the reserve was released.",
  sr_no_maker:"No eligible counterparty; the reserve was released.",
  sr_price_limit:"Price outside the limit; the reserve was released.",
  sr_automatic_dvp:"The ledger moved both pre-reserved legs at once.",
  price:"price",
  makerTitle:"Quoting policy",
  makerWhy:"You never see the order. Register a policy and a maximum size; the hidden order is evaluated against it.",
  ask_level:"level offset", spread:"spread", slope:"size charge", invcoef:"skew weight",
  inv:"inventory skew", maxqty:"max size", active:"active", assetLabel:"market",
  invWhy:"+ lifts both quotes (wants to buy back), − drops them (wants to sell).",
  reserveTitle:"Reserve (derived from the policy)",
  reserveWhy:"Inventory for sells up to max size, and cash for buys, are locked when the policy is registered.",
  inventoryReserve:"inventory to sell", cashReserve:"cash to buy",
  fillTitle:"Fill notice", noFill:"Nothing was said to you this round.",
  noFillWhy:"Whether you lost or the order was in another market, you cannot tell.",
  yourFill:"you were filled",
  tryTitle:"Price preview (local only)",
  tryWhy:"This runs in your browser. Nothing is sent.",
  tryQty:"size", ask:"ask", bid:"bid", on:"on", off:"off",
  nodeTitle:"Held data",
  nodeWhy:"Split values of the order and policies. This node alone cannot rebuild the originals.",
  noCustody:"Nodes hold no inventory or cash. They compute on split values and see only the verification status and the ledger hash.",
  behaviourTitle:"Node behaviour",
  behaviourWhy:"Your choice is visible only to you. Only the result becomes public.",
  verdictTitle:"This round",
  namedYou:"your misbehaviour was detected", namedNobody:"no misbehaviour detected",
  corrected:"openings corrected", capacityProduct:"correction capacity (multiplication)", capacityOpen:"correction capacity (opening)",
  rejectedYou:"your input did not match the dealing record. Excluded before computation.",
  refused:"excluded",
  b_honest:"compute honestly", b_honest_d:"nothing happens. Baseline.",
  b_lie_product:"send a wrong value in a multiplication",
  b_lie_product_d:"detected, corrected, the computation continues — up to capacity.",
  b_lie_open:"send a wrong value at the final reveal", b_lie_open_d:"detected and corrected.",
  b_lie_input:"use a value you were not dealt",
  b_lie_input_d:"with the input check on, caught before computation. Off — nobody notices and the answer is wrong.",
  b_dropout:"go silent after inputs", b_dropout_d:"reduces correction capacity.",
  b_offline:"never take part", b_offline_d:"a missing input destroys the value.",
  inertTitle:"External engine running", inertWhy:"this behaviour is not carried by the engine.",
  verifiedYes:"matches reference", verifiedNo:"does NOT match reference",
  protocolMs:"compute time", engRounds:"rounds", engMb:"traffic (MB)", compiledOnce:"compiled once",
  observerTitle:"Full view (demo only)",
  observerWhy:"Not present in production. Full-information demo display.",
  allQuotes:"all quotes", maker:"maker", node:"node",
  reason:"reason", request:"order", behaviours:"node behaviour", settings:"settings",
  roundEvery:"auto round interval (s)", stepMs:"time per phase (ms)",
  autoRounds:"auto rounds", inputCheck:"input check",
  inputCheckWhy:"turn off and nothing stops a node feeding a value it was not dealt.",
  runNow:"run one now",
  r_busy:"A round is in progress.",
  r_taker_cash:(f)=>`Not enough cash: ${f.have} available, ${f.need} needed at the limit price.`,
  r_taker_inv:(f)=>`Not enough inventory: ${f.have} units available, the order needs ${f.need}.`,
  r_maker_reserve:(f)=>`Cannot reserve the policy maximum: needs ${f.units} units and ${f.cash} cash.`,
  r_taken:(f)=>`${f.seat} is taken by ${f.who}.`,
  n_finished:(f)=>`#${f.number} done (${f.real?"live":"dummy"})`,
  n_corrected:(f)=>`#${f.number} corrected ${f.corrections} of ${f.reductions} openings; named ${nodeList(f.named)}`,
  n_stopped:(f)=>`#${f.number} stopped — ` + abortWhy(f),
  n_refused:(f)=>`${nodeList(f.who)} excluded`,
  n_unchecked:(f)=>`${nodeList(f.who)} used a value not dealt. Check is off — nobody noticed. Result is wrong`,
  n_claimed:(f)=>`${f.seat} taken${f.label?" by "+f.label:""}`,
  n_you_won:(f)=>`#${f.number} you were filled`,
  n_settled:(f)=>`#${f.number} settled without a further signature`,
  n_reserve_released:(f)=>`#${f.number} ${t('sr_' + (f.reason||'no_maker'))}`,
  n_limit_released:(f)=>`#${f.number} released — price outside the limit`,
  a_beyond_capacity:(f)=>`${f.answered} nodes answered; capacity is ${f.capacity}`,
  a_absent:(f)=>`${nodeList(f.who)} absent; all ${f.n} inputs are needed`,
  a_commitment:(f)=>`${nodeList(f.who)} failed the input check`,
  a_too_few:(f)=>`${f.answered} nodes are not enough (need ${f.needed})`,
  a_mismatch:(f)=>`result does not match the reference${f.detail?": "+f.detail:""}`,
  a_engine:(f)=>`the external engine failed${f.detail?": "+f.detail:""}`,
  explain:[
    ["Taker","sends the order. It is split so no single node sees it all. Only this seat gets the price."],
    ["Maker","registers a price policy and a maximum size; never sees the order. A win settles from the reserve automatically."],
    ["Node","computes on split values. Holds no inventory or cash. Can misbehave on purpose."]
  ]
}};

const hasDom = typeof document !== 'undefined';
let lang = 'ja';
if (hasDom){
  const fromUrl = new URLSearchParams(location.search).get('lang');
  let stored = null;
  try { stored = localStorage.getItem('qomm.lang'); } catch (e) { stored = null; }
  lang = fromUrl || stored
    || ((navigator.languages || [navigator.language || 'ja'])
          .some(l => String(l).toLowerCase().startsWith('ja')) ? 'ja' : 'en');
  if (lang !== 'ja' && lang !== 'en') lang = 'ja';
}
function t(k){ return (S[lang] && S[lang][k]) !== undefined ? S[lang][k] : (S.ja[k] !== undefined ? S.ja[k] : k); }
function tf(k, fields){ const f = t(k); return typeof f === 'function' ? f(fields || {}) : String(f); }
function nodeList(list){
  const items = Array.isArray(list) ? list : [];
  const word = lang === 'ja' ? 'ノード' : 'node ';
  return items.map(n => word + n).join(lang === 'ja' ? '・' : ', ');
}
function msg(code, fields){
  const f = t('n_' + code);
  if (typeof f === 'function'){ try { return f(fields || {}); } catch (e) { /* fall through */ } }
  return code + ' ' + JSON.stringify(fields || {});
}
function abortWhy(fields){
  const f = t('a_' + (fields.why || fields.code || ''));
  const merged = Object.assign({}, fields.fields || {}, fields);
  if (typeof f === 'function'){ try { return f(merged); } catch (e) { /* fall through */ } }
  return fields.detail || '';
}
function abortText(pub){
  if (!pub || !pub.aborted) return '';
  return abortWhy(Object.assign({why: pub.abort_code, detail: pub.abort_reason},
                                pub.abort_fields || {}));
}
function translateRefusal(reason){
  const r = String(reason || '');
  let m;
  if (/already in progress/.test(r)) return t('r_busy');
  if ((m = /Taker has (\d+) cash units available but the signed limit needs (\d+)/.exec(r)))
    return tf('r_taker_cash', {have: showCash(m[1]), need: showCash(m[2])});
  if ((m = /Taker has (\d+) units available but the request needs (\d+)/.exec(r)))
    return tf('r_taker_inv', {have: showCash(m[1]), need: showCash(m[2])});
  if ((m = /cannot reserve policy maximum: needs (\d+) units and (\d+) cash units/.exec(r)))
    return tf('r_maker_reserve', {units: showCash(m[1]), cash: showCash(m[2])});
  if ((m = /^(\S+) is taken by (.+)$/.exec(r)))
    return tf('r_taken', {seat: m[1], who: m[2]});
  return r;
}

/* ========================================================================= */
/*  State                                                                     */
/* ========================================================================= */
const BEHAVIOURS = ['honest','lie_product','lie_open','lie_input','dropout','offline'];
const PHASE_ORDER = ['idle','deal','check','reduce','open','settle','done'];
const PHASE_STEPS = [['deal','phaseDeal'],['check','phaseCheck'],['reduce','phaseReduce'],
                     ['open','phaseOpen'],['settle','phaseSettle']];
const POLICY_RANGE = {ask_level:[-40,40], spread:[6,120], slope:[0,4], invcoef:[0,3],
                      inv:[-120,120], maxqty:[0,500]};
const ASSET_ALIASES = {
  'USD/JPY': ['usd/jpy','usdjpy','usd-jpy','ドル円','ドル/円','米ドル円','ドルえん'],
  'EUR/USD': ['eur/usd','eurusd','eur-usd','ユーロドル','ユーロ/ドル'],
  'BTC/USD': ['btc/usd','btcusd','btc-usd','ビットコイン','btc','ビット']
};

let ws = null, V = null, built = '', lastDraw = '', lastError = '', lastErrorTone = 'bad';
let session = '';
if (hasDom){ try { session = localStorage.getItem('qomm.session') || ''; } catch (e) { session = ''; } }
let chatHistory = [], chatPending = null, chatAwaitingReply = false;
let pfPrev = null, pfPrevRound = null, pfRoundBaseline = null;
let pfDelta = {}, pfDeltaPhase = '', pfDeltaKind = '';
let reducedMotion = false, lastGraphWidth = -1;
if (hasDom && window.matchMedia){
  const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
  reducedMotion = mq.matches;
  const onChange = () => { reducedMotion = mq.matches; lastDraw = ''; render(); };
  if (mq.addEventListener) mq.addEventListener('change', onChange);
  else if (mq.addListener) mq.addListener(onChange);
}

const $ = (id) => document.getElementById(id);
const el = (tag, cls, txt) => { const e = document.createElement(tag);
  if (cls) e.className = cls; if (txt !== undefined && txt !== null) e.textContent = txt; return e; };
const NS = 'http://www.w3.org/2000/svg';
const svgEl = (tag, attrs) => { const e = document.createElementNS(NS, tag);
  Object.keys(attrs || {}).forEach(k => e.setAttribute(k, attrs[k])); return e; };

function send(message){
  if (ws && ws.readyState === 1){ ws.send(JSON.stringify(message)); return true; }
  return false;
}
function setError(text, tone){
  lastError = text || ''; lastErrorTone = tone || 'bad';
  const bar = $('errorbar');
  if (!bar) return;
  bar.textContent = lastError;
  bar.classList.toggle('hide', !lastError);
  bar.classList.toggle('warn', lastErrorTone === 'warn');
}

function connect(){
  const q = new URLSearchParams();
  if (session) q.set('session', session);
  const url = new URLSearchParams(location.search);
  if (url.get('seat')) q.set('seat', url.get('seat'));
  if (url.get('label')) q.set('label', url.get('label'));
  $('conn').textContent = t('connecting');
  ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://')
                     + location.host + '/ws?' + q.toString());
  ws.onmessage = (e) => {
    let m; try { m = JSON.parse(e.data); } catch (err) { return; }
    if (m.type === 'view'){
      V = m; session = m.session;
      try { localStorage.setItem('qomm.session', session); } catch (err) { /* private mode */ }
      if (lastErrorTone === 'warn') setError('', 'bad');
      render();
    } else if (m.type === 'refused'){
      const text = translateRefusal(m.reason || 'request refused');
      setError(text, 'bad');
      if (chatAwaitingReply){ addChatMessage(tf('chatRefused', text), 'error'); chatAwaitingReply = false; }
    }
  };
  ws.onclose = () => {
    $('conn').textContent = t('reconnecting');
    setError(t('reconnecting'), 'warn');
    setTimeout(connect, 1200);
  };
  ws.onerror = () => { /* onclose follows and schedules the retry */ };
  ws.onopen = () => { $('conn').textContent = ''; };
}

/* ─── number helpers ─────────────────────────────────────────────────── */
function assetScale(assets, index){ const a = (assets || [])[index]; return a ? a.scale : 1; }
function showPrice(assets, index, ticks){
  if (ticks === null || ticks === undefined) return '--';
  const a = (assets || [])[index]; if (!a) return String(ticks);
  const digits = String(a.scale).length - 1;
  return (ticks / a.scale).toLocaleString(undefined,
    {minimumFractionDigits: digits, maximumFractionDigits: digits});
}
function showCash(value){
  if (value === null || value === undefined || value === '') return '--';
  return Number(value).toLocaleString();
}
function shortRoot(root){ return root ? String(root).slice(0, 8) + '…' : '--'; }
function showQuoteReason(reason){
  if (!reason || lang !== 'ja') return reason || '';
  const fixed = {
    'switched off':'停止中', 'different market':'別の銘柄',
    'policy expired':'有効期限切れ',
    'pre-reserve belongs to another asset':'予約が別の銘柄',
    'pre-reserved inventory is too small':'売却用在庫が不足',
    'pre-reserved cash is too small':'買付用資金が不足'
  };
  if (fixed[reason]) return fixed[reason];
  const size = /^size (\d+) above its limit (\d+)$/.exec(reason);
  return size ? `数量${size[1]}が上限${size[2]}超` : reason;
}
function metric(key, value){
  const box = el('div', 'metric');
  box.appendChild(el('div', 'k', key));
  box.appendChild(el('div', 'v', value));
  return box;
}
/* The maker's cash reserve is the most any buy-back under the policy could
   cost, computed exactly as the room computes it (rust/qomm-demo/src/room.rs
   maker_cash_requirement), so a chat preview can state the number the server
   will lock. */
function makerCashRequirement(policy, assets){
  if (!policy || policy.active === 0 || policy.maxqty <= 0) return 0;
  const ref = (assets[policy.asset] || {reference: 0}).reference;
  const anchor = policy.use_ref * ref + policy.ask_level;
  const skew = policy.invcoef * policy.inv;
  let max = 0;
  for (let q = 1; q <= policy.maxqty; q++){
    const bid = anchor - policy.spread - policy.slope * q + skew;
    max = Math.max(max, q * Math.max(bid, 0));
  }
  return max;
}
function settlementName(status){
  return status === 'settled' ? t('settled')
    : status === 'released' ? t('released')
    : status === 'cover' ? t('coverStatus') : (status || '--');
}
function drawSettlement(box, settlement){
  box.textContent = '';
  if (!settlement){ box.appendChild(el('p', 'empty', t('noSettlement'))); return; }
  box.appendChild(el('span', 'status-pill ' + settlement.status,
    settlementName(settlement.status)));
  const grid = el('div', 'settlement-grid');
  grid.appendChild(metric(t('qty'), showCash(settlement.quantity)));
  grid.appendChild(metric(t('limitPrice'), showPrice(V.assets, settlement.asset,
    settlement.limit_price)));
  if (settlement.price !== null && settlement.price !== undefined)
    grid.appendChild(metric(t('price'), showPrice(V.assets, settlement.asset, settlement.price)));
  if (settlement.cash !== null && settlement.cash !== undefined)
    grid.appendChild(metric(t('settlementCash'), showCash(settlement.cash)));
  box.appendChild(grid);
  if (settlement.automatic) box.appendChild(el('div', 'hidden-box', t('automatic')));
  const reason = t('sr_' + settlement.reason_code);
  if (reason && reason !== 'sr_' + settlement.reason_code) box.appendChild(el('p', 'why', reason));
  if (settlement.detail) box.appendChild(el('p', 'why mono', settlement.detail));
  if (settlement.state_root){
    box.appendChild(el('div', 'tag', t('stateRoot')));
    box.appendChild(el('div', 'root', settlement.state_root));
  }
}
function setVal(node, value){
  if (node && document.activeElement !== node) node.value = value;
}
function phaseIndex(phase){ return PHASE_ORDER.indexOf(phase); }
function isBusy(){ return !!(V && (V.busy || (V.phase !== 'idle' && V.phase !== 'done'))); }
/* Where an aborted round stopped, from the public abort code. */
function stoppedAt(pub){
  if (!pub || !pub.aborted) return null;
  const code = pub.abort_code || '';
  if (code === 'absent' || code === 'commitment') return 'check';
  return 'reduce';
}

/* ========================================================================= */
/*  Phase strip and caption                                                   */
/* ========================================================================= */
function phaseCaption(){
  const f = V.phase_fields || {}, pub = V.public || {};
  switch (V.phase){
    case 'deal': return tf('ph_deal', f);
    case 'check':
      if (f.aborted && f.why === 'absent') return abortWhy(f);
      if (f.skipped) return t('ph_check_skipped');
      if (f.rejected && f.rejected.length) return tf('ph_check_rejected', f);
      return tf('ph_check', f);
    case 'reduce': {
      if (f.aborted && !(f.corrections > 0)) return abortWhy(f);
      const st = f.engine_stats || {};
      let text = f.engine === 'mpc' ? tf('ph_reduce_mpc', {rounds: st.rounds, mb: st.mb}) : tf('ph_reduce', f);
      if (f.corrections > 0) text += tf('ph_reduce_corrected', f);
      return text;
    }
    case 'open': return t('ph_open');
    case 'settle':
      if (f.status === 'settled') return t('ph_settle_settled');
      if (f.status === 'cover') return t('ph_settle_cover');
      return tf('ph_settle_released', {reason: t('sr_' + (f.reason || 'no_maker'))});
    case 'done': {
      let text = tf('ph_done', {number: f.number || pub.number || '--'});
      if (pub.aborted) text += ' ' + t('stopped') + ' — ' + abortText(pub);
      return text;
    }
    default:
      return V.next_round_in === null || V.next_round_in === undefined ? t('idleManual') : t('idleAuto');
  }
}
function drawPhaseStrip(){
  const strip = $('phase-strip'); strip.textContent = '';
  const pub = V.public || {};
  const at = phaseIndex(V.phase);
  const stop = stoppedAt(pub);
  const stopIndex = stop ? phaseIndex(stop) : -1;
  PHASE_STEPS.forEach(([id, key]) => {
    const i = phaseIndex(id);
    let cls = 'phase-step';
    if (V.phase === id) cls += ' on';
    else if (at > i && (V.phase !== 'idle')) cls += ' done';
    if (stop && (V.phase === 'settle' || V.phase === 'done' || at > i) && i === stopIndex) cls += ' stopped';
    if (stop && i > stopIndex && i < phaseIndex('settle')) cls = 'phase-step';
    const step = el('span', cls, t(key)); step.setAttribute('role', 'listitem');
    if (V.phase === id) step.setAttribute('aria-current', 'step');
    strip.appendChild(step);
  });
  const done = el('span', 'phase-step' + (V.phase === 'done' ? ' on' : ''), t(V.phase === 'idle' ? 'phaseIdle' : 'phaseDone'));
  done.setAttribute('role', 'listitem');
  strip.appendChild(done);
  const caption = $('phase-caption'); caption.textContent = phaseCaption();
  if ((V.phase === 'done' || V.phase === 'settle') && pub.ms !== undefined){
    caption.appendChild(document.createTextNode(' '));
    caption.appendChild(el('span', 'ms', tf('computeMs', pub.ms)));
  }
}

/* ========================================================================= */
/*  Network graph: nodes as HTML, edges as SVG, driven by V.phase             */
/* ========================================================================= */
function orthoPath(points, radius){
  const pts = points.filter((p, i) => i === 0 || p.x !== points[i-1].x || p.y !== points[i-1].y);
  if (pts.length < 2) return '';
  const r = radius === undefined ? 10 : radius;
  let d = `M${pts[0].x},${pts[0].y}`;
  for (let i = 1; i < pts.length - 1; i++){
    const p = pts[i-1], c = pts[i], n = pts[i+1];
    const d1 = Math.hypot(c.x - p.x, c.y - p.y), d2 = Math.hypot(n.x - c.x, n.y - c.y);
    const rr = Math.min(r, d1 / 2, d2 / 2);
    const a = {x: c.x + (p.x - c.x) / d1 * rr, y: c.y + (p.y - c.y) / d1 * rr};
    const b = {x: c.x + (n.x - c.x) / d2 * rr, y: c.y + (n.y - c.y) / d2 * rr};
    d += ` L${a.x},${a.y} Q${c.x},${c.y} ${b.x},${b.y}`;
  }
  const last = pts[pts.length - 1];
  d += ` L${last.x},${last.y}`;
  return d;
}
function edgeId(from, to){ return 'e-' + (from + '-' + to).replace(/[^a-z0-9-]/gi, '-'); }

/* Everything the diagram needs from V for this seat: what each node should
   say, what it may say, which edges are the seat's own. */
function graphModel(W){
  const cfg = V.config || {}, pub = V.public || {}, kind = V.kind;
  const nMakers = cfg.n_makers || 0, nNodes = cfg.n_nodes || 0;
  const me = V.seat && kind !== 'observer' ? kind + (kind === 'taker' ? '' : ':' + V.index) : null;
  const compact = W < 560, wide = W >= 760;
  const padX = compact ? 18 : 26, gap = compact ? 2 : 8;
  const phase = V.phase, at = phaseIndex(phase);
  const stop = stoppedAt(pub);
  const stopIndex = stop ? phaseIndex(stop) : 99;
  const busy = isBusy();
  const taker = V.taker || null, maker = V.maker || null, node = V.node || null, obs = V.observer || null;
  const named = new Set(pub.named || []), silent = new Set(pub.silent || []);
  const rejected = new Set((pub.rejected || []).map(r => r.node));
  const nodes = [], edges = [], labels = [];

  // ── row A: taker at the left, makers across the rest ──
  const rowA = 32;
  const tw = compact ? 70 : 122, th = compact ? 36 : 42;
  const takerX = padX + tw / 2;
  const makerAreaL = padX + tw + gap * 2, makerAreaR = W - padX;
  const mw = Math.max(compact ? 22 : 30, Math.min(compact ? 44 : 92, (makerAreaR - makerAreaL - gap * (nMakers - 1)) / Math.max(nMakers, 1)));
  const mh = compact ? 30 : 38;
  const makerSpan = mw * nMakers + gap * (nMakers - 1);
  const makerStart = makerAreaL + Math.max(0, (makerAreaR - makerAreaL - makerSpan) / 2);
  // ── row B: nodes across the full width, wrapping when there are many ──
  const perRow = Math.max(1, Math.floor((W - 2 * padX + gap) / ((compact ? 34 : 60) + gap)));
  const nodeRows = Math.max(1, Math.ceil(nNodes / perRow));
  const nPerRow = Math.ceil(nNodes / nodeRows);
  const nw = Math.max(30, Math.min(compact ? 44 : 80, (W - 2 * padX - gap * (nPerRow - 1)) / nPerRow));
  const nh = compact ? 30 : 38;
  const rowB = rowA + th / 2 + 64 + nh / 2;
  const rowBBottom = rowB + (nodeRows - 1) * (nh + 10) + nh / 2;
  // ── row C: matcher → verify → ledger ──
  const cw = compact ? 92 : 124, ch = compact ? 40 : 46;
  const lw = compact ? 150 : 190;
  let rowC, rowD, rowE;
  if (wide){ rowC = rowD = rowE = rowBBottom + 66 + ch / 2; }
  else { rowC = rowBBottom + 58 + ch / 2; rowD = rowC + ch + 30; rowE = rowD + ch + 30; }
  const H = rowE + ch / 2 + 22;

  const add = (n) => { nodes.push(n); return n; };
  add({id: 'taker', type: 'taker', x: takerX, y: rowA, w: tw, h: th});
  for (let i = 0; i < nMakers; i++)
    add({id: 'maker:' + i, type: 'maker', index: i, x: makerStart + i * (mw + gap) + mw / 2, y: rowA, w: mw, h: mh});
  for (let i = 0; i < nNodes; i++){
    const r = Math.floor(i / nPerRow), c = i % nPerRow;
    const count = Math.min(nPerRow, nNodes - r * nPerRow);
    const span = nw * count + gap * (count - 1);
    const start = padX + (W - 2 * padX - span) / 2;
    add({id: 'node:' + i, type: 'node', index: i, x: start + c * (nw + gap) + nw / 2, y: rowB + r * (nh + 10), w: nw, h: nh});
  }
  const matcher = add({id: 'matcher', type: 'matcher', x: wide ? W * 0.22 : W / 2, y: rowC, w: cw, h: ch});
  const zkpi = add({id: 'zkpi', type: 'zkpi', x: wide ? W * 0.5 : W / 2, y: rowD, w: cw, h: ch});
  const ledger = add({id: 'ledger', type: 'ledger', x: wide ? W * 0.78 : W / 2, y: rowE, w: lw, h: ch});
  const byId = {}; nodes.forEach(n => { byId[n.id] = n; });
  const top = (n) => ({x: n.x, y: n.y - n.h / 2}), bottom = (n) => ({x: n.x, y: n.y + n.h / 2});
  const left = (n) => ({x: n.x - n.w / 2, y: n.y}), right = (n) => ({x: n.x + n.w / 2, y: n.y});
  const laneL = 12, laneL2 = 18, laneR = W - 12;
  const trunkY = rowA + th / 2 + 30, returnY = rowA + th / 2 + 14;
  const trunk2Y = rowBBottom + 26;
  const bottomLane = rowE + ch / 2 + 12;

  // ── who knows what ──
  const knowsOrder = kind === 'taker' || kind === 'observer';
  const lastOrder = kind === 'taker' ? (taker && taker.last) : kind === 'observer' ? obs && obs.request : null;
  const pendingOrder = kind === 'taker' ? (taker && taker.pending) : null;
  const winnerKnown = kind === 'taker' ? (taker && taker.last && taker.last.winner !== null && taker.last.winner !== undefined ? taker.last.winner : null)
    : kind === 'observer' ? (obs && obs.winner !== null && obs.winner !== undefined ? obs.winner : null)
    : kind === 'maker' && maker && maker.fill ? V.index : null;
  const settlement = pub.settlement || null;
  const settleStatus = phase === 'settle' ? (V.phase_fields || {}).status : (settlement ? settlement.status : null);
  const roundExists = !!pub.number;
  const stageState = (stage) => {   // 'idle' | 'flow' | 'done' | 'cut'
    const s = phaseIndex(stage);
    if (!roundExists && phase === 'idle') return 'idle';
    if (stop && s > stopIndex && stage !== 'settle') return 'cut';
    if (phase === stage) return 'flow';
    if (at > s || phase === 'done') return 'done';
    return 'idle';
  };
  const mine = (id) => kind === 'observer' || id === me;

  // ── edges: split values from every participant to every node ──
  const participants = ['taker'].concat(Array.from({length: nMakers}, (_, i) => 'maker:' + i));
  const dealState = stageState('deal');
  participants.forEach(p => {
    const from = byId[p];
    for (let i = 0; i < nNodes; i++){
      const to = byId['node:' + i];
      const own = kind === 'node' ? (i === V.index) : mine(p);
      edges.push({id: edgeId(p, to.id), stage: 'deal', state: dealState, own, color: 'teal',
        d: orthoPath([bottom(from), {x: from.x, y: trunkY}, {x: to.x, y: trunkY}, top(to)]),
        chain: [0, 1], particle: own && (kind !== 'observer' || p === 'taker')});
    }
  });
  if (dealState === 'flow'){
    const text = kind === 'taker' ? t('eMyOrder') : kind === 'maker' ? t('eMyPolicy') : t('eShares');
    labels.push({x: takerX + tw / 2 + 6, y: trunkY - 6, text, anchor: 'start', strong: true});
  }
  // ── nodes to the matcher ──
  const reduceState = stageState('reduce');
  for (let i = 0; i < nNodes; i++){
    const from = byId['node:' + i];
    const own = kind === 'node' ? i === V.index : true;
    const dead = silent.has(i) || rejected.has(i);
    edges.push({id: edgeId(from.id, 'matcher'), stage: 'reduce', state: dead ? 'cut' : reduceState, own, color: 'teal',
      d: orthoPath([bottom(from), {x: from.x, y: trunk2Y}, {x: matcher.x, y: trunk2Y}, top(matcher)]),
      chain: [0, 1], particle: own && kind !== 'observer' || (kind === 'observer' && i === 0)});
  }
  if (reduceState === 'flow') labels.push({x: matcher.x, y: trunk2Y - 6, text: t('eCompute'), strong: true});
  // ── the keyed result back to the taker (everyone sees it; only the taker reads it) ──
  const openState = stageState('open');
  const takerN = byId.taker;
  edges.push({id: edgeId('matcher', 'taker'), stage: 'open', state: openState, own: true, color: 'amber',
    d: orthoPath([left(matcher), {x: laneL, y: matcher.y}, {x: laneL, y: takerN.y}, left(takerN)]),
    chain: [0, 1], particle: true});
  if (openState === 'flow') labels.push({x: laneL + 6, y: (matcher.y + takerN.y) / 2, text: t('eOpen'), anchor: 'start', strong: true});
  // ── verify → ledger → participants ──
  const settleState = stageState('settle');
  const aborted = !!pub.aborted;
  const chainLen = 3;
  edges.push({id: edgeId('matcher', 'zkpi'), stage: 'settle', state: aborted ? 'cut' : settleState, own: true, color: 'blue',
    d: wide ? orthoPath([right(matcher), left(zkpi)]) : orthoPath([bottom(matcher), top(zkpi)]),
    chain: [0, chainLen], particle: true});
  edges.push({id: edgeId('zkpi', 'ledger'), stage: 'settle', state: aborted ? 'cut' : settleState, own: true, color: 'blue',
    d: wide ? orthoPath([right(zkpi), left(ledger)]) : orthoPath([bottom(zkpi), top(ledger)]),
    chain: [1, chainLen], particle: true});
  if (settleState === 'flow' && !aborted){
    labels.push(wide ? {x: (matcher.x + zkpi.x) / 2, y: rowC - 8, text: t('eVerify')}
                     : {x: matcher.x + 8, y: (rowC + rowD) / 2 + 4, text: t('eVerify'), anchor: 'start'});
    labels.push(wide ? {x: (zkpi.x + ledger.x) / 2, y: rowC - 8, text: t('eLedger')}
                     : {x: zkpi.x + 8, y: (rowD + rowE) / 2 + 4, text: t('eLedger'), anchor: 'start'});
  }
  // ledger → taker: settle, release, or nothing to change
  const takerSettleText = settleStatus === 'settled' ? t('eSettleTaker')
    : settleStatus === 'released' ? t('eRelease') : settleStatus === 'cover' ? t('eNoChange') : '';
  const ledgerToTaker = wide
    ? orthoPath([bottom(ledger), {x: ledger.x, y: bottomLane}, {x: laneL2, y: bottomLane}, {x: laneL2, y: takerN.y + 8}, {x: takerN.x - tw / 2, y: takerN.y + 8}])
    : orthoPath([left(ledger), {x: laneL2, y: ledger.y}, {x: laneL2, y: takerN.y + 8}, {x: takerN.x - tw / 2, y: takerN.y + 8}]);
  edges.push({id: edgeId('ledger', 'taker'), stage: 'settle', state: settleStatus === 'cover' && settleState !== 'idle' ? 'done' : settleState, own: true,
    color: settleStatus === 'released' ? 'amber' : 'blue', d: ledgerToTaker, chain: [2, chainLen], particle: settleStatus !== 'cover'});
  if (settleState === 'flow' && takerSettleText)
    labels.push({x: laneL2 + 6, y: wide ? (bottomLane + takerN.y) / 2 : (ledger.y + takerN.y) / 2 + 14, text: takerSettleText, anchor: 'start', strong: true});
  // ledger → the winning maker, only where this seat may know who won
  if (winnerKnown !== null && byId['maker:' + winnerKnown] && settleStatus === 'settled'){
    const m = byId['maker:' + winnerKnown];
    edges.push({id: edgeId('ledger', m.id), stage: 'settle', state: settleState, own: true, color: 'blue',
      d: orthoPath([right(ledger), {x: laneR, y: ledger.y}, {x: laneR, y: returnY}, {x: m.x, y: returnY}, bottom(m)]),
      chain: [2, chainLen], particle: true});
    if (settleState === 'flow') labels.push({x: laneR - 6, y: (ledger.y + returnY) / 2, text: t('eSettleMaker'), anchor: 'end', strong: true});
  }

  // ── what each node says ──
  const orderText = (o) => o ? `${(V.assets[o.asset] || {}).name || '--'} ${o.direction ? t('sell') : t('buy')} ${o.qty}${o.is_real ? '' : ' · ' + t('cover')}` : '';
  nodes.forEach(n => {
    n.classes = []; n.title = ''; n.sub = ''; n.badge = '';
    const isMe = n.id === me;
    if (isMe) n.classes.push('is-me');
    if (n.type === 'taker'){
      n.title = t('gTaker') + (isMe && !compact ? ' · ' + t('you') : '');
      if (knowsOrder){
        const shown = (phase === 'idle' || phase === 'done') && pendingOrder && kind === 'taker' && !busy ? pendingOrder : (lastOrder || pendingOrder);
        n.sub = orderText(shown) || t('orderHidden');
      } else n.sub = t('orderHidden');
      if (dealState === 'flow' || openState === 'flow' || (settleState === 'flow' && settleStatus !== 'cover')) n.classes.push('is-active');
      // The price badge needs room beside the title; a compact node has none,
      // and the price is in the panel and the strip anyway.
      if (!compact && kind === 'taker' && taker && taker.last && phase === 'done' && !pub.aborted && taker.last.winner !== null && taker.last.winner !== undefined && taker.last.is_real)
        n.badge = showPrice(V.assets, taker.last.asset, taker.last.price);
    } else if (n.type === 'maker'){
      // "you" goes in the second line: the title has no room beside the
      // index, and the ring around the seat's own node already says it.
      n.title = (compact ? 'M' : t('gMaker') + ' ') + n.index;
      const pol = kind === 'maker' && isMe ? maker && maker.policy : kind === 'observer' ? (obs && obs.policies || [])[n.index] : null;
      if (pol){
        n.sub = `${(V.assets[pol.asset] || {}).name || '--'} · ${pol.active ? t('activeShort') : t('inactiveShort')} · ${t('maxShort')} ${pol.maxqty}`;
        if (!pol.active) n.classes.push('is-off');
      } else n.sub = t('policyHidden');
      if (isMe) n.sub = t('you') + ' · ' + n.sub;
      if (dealState === 'flow' && (mine(n.id) || kind !== 'maker')) n.classes.push('is-active');
      if (winnerKnown === n.index && (phase === 'settle' || phase === 'done') && settleStatus === 'settled') n.classes.push('is-winner');
      if (compact) n.classes.push('compact');
    } else if (n.type === 'node'){
      n.title = (compact ? 'N' : t('gNode') + ' ') + n.index;
      const live = !silent.has(n.index) && !rejected.has(n.index);
      let sub = t('nIdle');
      if (silent.has(n.index)) { sub = t('nSilent'); n.classes.push('is-silent'); }
      else if (rejected.has(n.index) && at >= phaseIndex('check')) { sub = t('nRejected'); n.classes.push('is-rejected'); }
      else if (named.has(n.index) && at >= phaseIndex('reduce')) { sub = t('nNamed'); n.classes.push('is-named'); }
      else if (phase === 'deal') sub = t('nReceiving');
      else if (phase === 'check') sub = t('nChecking');
      else if (phase === 'reduce') sub = t('nComputing');
      else if (at >= phaseIndex('open')) sub = stop && stopIndex <= phaseIndex('reduce') ? t('nStopped') : t('nDone');
      if (isMe && node && node.behaviour && node.behaviour !== 'honest') sub = t('b_' + node.behaviour);
      n.sub = isMe ? t('you') + ' · ' + sub : sub;
      if (live && (phase === 'deal' || phase === 'check' || phase === 'reduce')) n.classes.push('is-active');
      if (compact) n.classes.push('compact');
    } else if (n.type === 'matcher'){
      n.title = t('gMatcher');
      if (phase === 'reduce') { n.sub = t('mCompare'); n.classes.push('is-active'); }
      else if (stop && stopIndex <= phaseIndex('reduce') && at >= stopIndex) { n.sub = t('nStopped'); n.classes.push('is-stopped'); }
      else if (at >= phaseIndex('open')){
        n.sub = kind === 'taker' && taker && taker.last ? (taker.last.winner === null || taker.last.winner === undefined ? t('mNone') : `${t('winner')}: ${t('gMaker')} ${taker.last.winner}`)
          : kind === 'observer' && obs ? (obs.winner === null || obs.winner === undefined ? t('mNone') : `${t('winner')}: ${t('gMaker')} ${obs.winner}`)
          : t('mPick');
        if (phase === 'open') n.classes.push('is-active');
      } else n.sub = t('nIdle');
    } else if (n.type === 'zkpi'){
      n.title = t('gZkpi');
      if (phase === 'settle' || phase === 'done'){
        n.sub = pub.aborted ? t('zStopped') : pub.verified === true ? t('zPass') : pub.verified === false ? t('zFail') : t('zDecoded');
        if (pub.aborted || pub.verified === false) n.classes.push('is-stopped');
        if (phase === 'settle' && !pub.aborted) n.classes.push('is-active');
      } else n.sub = t('zWait');
    } else if (n.type === 'ledger'){
      n.title = t('gLedger');
      const root = phase === 'settle' ? (V.phase_fields || {}).state_root : settlement && settlement.state_root;
      if (settleStatus){
        n.sub = (settleStatus === 'settled' ? t('lSettled') : settleStatus === 'released' ? t('lReleased') : t('lCover')) + (root ? ' · ' + shortRoot(root) : '');
      } else n.sub = t('lWait');
      if (phase === 'settle') n.classes.push('is-active');
    }
  });
  return {nodes, edges, labels, W, H, compact};
}

function drawNetworkGraph(){
  const container = $('network-graph'), svg = $('network-svg');
  const edgesG = $('graph-edges'), particlesG = $('graph-particles'), labelsG = $('graph-labels');
  const nodesDiv = $('network-nodes'), legendDiv = $('graph-legend'), empty = $('graph-empty');
  if (!container || !svg || !V) return;
  const W = Math.max(300, container.clientWidth || 700);
  lastGraphWidth = container.clientWidth;
  const model = graphModel(W);
  container.style.height = model.H + 'px';
  svg.setAttribute('width', W); svg.setAttribute('height', model.H);
  svg.setAttribute('viewBox', `0 0 ${W} ${model.H}`);
  container.setAttribute('aria-label', t('graphAria'));
  edgesG.textContent = ''; particlesG.textContent = ''; labelsG.textContent = ''; nodesDiv.textContent = '';

  // faint edges first so the seat's own paths draw on top
  const order = model.edges.slice().sort((a, b) => (a.own === b.own ? 0 : a.own ? 1 : -1));
  order.forEach(e => {
    let cls = 'gedge';
    if (!e.own) cls += ' faint';
    if (e.state === 'flow') cls += ' is-flow ' + e.color;
    else if (e.state === 'done') cls += ' is-done';
    else if (e.state === 'cut') cls += ' is-off';
    const path = svgEl('path', {id: e.id, d: e.d, class: cls});
    if (e.state === 'flow') path.setAttribute('marker-end', 'url(#arrow-flow)');
    else if (e.state === 'done') path.setAttribute('marker-end', 'url(#arrow-done)');
    else if (e.own) path.setAttribute('marker-end', 'url(#arrow)');
    edgesG.appendChild(path);
    if (e.state === 'flow' && e.own && e.particle && !reducedMotion){
      const [k, K] = e.chain;
      const dot = svgEl('circle', {class: 'gparticle ' + e.color, r: 4});
      const motion = svgEl('animateMotion', {dur: (K * 1.1) + 's', repeatCount: 'indefinite', calcMode: 'linear',
        keyPoints: K > 1 ? `0;0;1;1` : '0;1', keyTimes: K > 1 ? `0;${(k / K).toFixed(3)};${((k + 1) / K).toFixed(3)};1` : '0;1'});
      const mpath = svgEl('mpath', {});
      mpath.setAttribute('href', '#' + e.id);
      mpath.setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', '#' + e.id);
      motion.appendChild(mpath); dot.appendChild(motion); particlesG.appendChild(dot);
    }
  });
  model.labels.forEach(l => {
    const text = svgEl('text', {x: l.x, y: l.y, class: 'gedge-label' + (l.strong ? ' strong' : '')});
    // the stylesheet centres labels; a lane label is anchored inline so the
    // rule does not override it
    if (l.anchor){ text.setAttribute('text-anchor', l.anchor); text.style.textAnchor = l.anchor; }
    text.textContent = l.text; labelsG.appendChild(text);
  });
  model.nodes.forEach(n => {
    const div = el('div', ['gnode', n.type].concat(n.classes).join(' '));
    div.style.left = (n.x - n.w / 2) + 'px'; div.style.top = (n.y - n.h / 2) + 'px';
    div.style.width = n.w + 'px'; div.style.height = n.h + 'px';
    div.setAttribute('tabindex', '0'); div.setAttribute('role', 'img');
    const full = n.title + (n.sub ? ' — ' + n.sub : '') + (n.badge ? ' ' + n.badge : '');
    div.setAttribute('aria-label', full); div.title = full;
    div.appendChild(el('div', 'gnode-title', n.title));
    if (n.sub) div.appendChild(el('div', 'gnode-sub', n.sub));
    if (n.badge) div.appendChild(el('div', 'gnode-badge', n.badge));
    nodesDiv.appendChild(div);
  });
  const noRound = !(V.public && V.public.number) && V.phase === 'idle';
  empty.classList.toggle('hide', !noRound);
  empty.textContent = noRound ? t('noRoundYet') + ' ' + phaseCaption() : '';
  legendDiv.textContent = '';
  [['taker','gTaker'],['maker','gMaker'],['node','gNode'],['matcher','gMatcher'],['zkpi','gZkpi'],['ledger','gLedger']]
    .forEach(([cls, key]) => legendDiv.appendChild(el('span', 'legend-chip ' + cls, t(key))));
  legendDiv.appendChild(el('span', 'legend-note', t('legendNoCustody')));
  legendDiv.appendChild(el('span', 'legend-note', t('legendLines')));
  legendDiv.appendChild(el('span', 'legend-note product-boundary', t('productBoundary')));
}

/* ========================================================================= */
/*  Balances strip                                                            */
/* ========================================================================= */
function portfolioSnapshot(p){
  const snap = {cash: p.cash.available, cashR: p.cash.reserved};
  (p.inventory || []).forEach(row => { snap['inv' + row.asset] = row.available; snap['invR' + row.asset] = row.reserved; });
  return snap;
}
function snapshotDiff(before, after){
  const changed = {};
  Object.keys(after).forEach(k => {
    if (before && before[k] !== undefined && before[k] !== after[k]) changed[k] = after[k] - before[k];
  });
  return changed;
}
function updateDeltas(portfolio){
  if (!portfolio) return;
  const snap = portfolioSnapshot(portfolio);
  const currentRound = V && V.public ? V.public.number : null;
  if (pfPrev){
    if (V.phase === 'deal' && !pfRoundBaseline) pfRoundBaseline = {...pfPrev};
    const settlement = V.phase === 'settle' ? (V.phase_fields || {})
      : (V.public && V.public.settlement) || {};
    const settledFromBaseline = pfRoundBaseline && settlement.status === 'settled'
      && (V.phase === 'settle' || V.phase === 'done');
    const released = settlement.status === 'released' && (V.phase === 'settle' || V.phase === 'done');
    const changed = snapshotDiff(settledFromBaseline ? pfRoundBaseline : pfPrev, snap);
    if (Object.keys(changed).length){
      pfDelta = changed;
      pfDeltaPhase = V.phase;
      const availableKeys = Object.keys(snap).filter(k => !k.endsWith('R'));
      let paired = 0, reserveMoves = 0, releaseMoves = 0, unpaired = 0;
      availableKeys.forEach(k => {
        const rk = k + 'R';
        const a = changed[k], r = changed[rk];
        if (a === undefined && r === undefined) return;
        if (a !== undefined && r !== undefined && a + r === 0){
          paired++;
          if (a < 0) reserveMoves++;
          else if (a > 0) releaseMoves++;
        } else unpaired++;
      });
      pfDeltaKind = settledFromBaseline ? 'settle' : released ? 'release'
        : paired > 0 && unpaired === 0
        ? (reserveMoves > 0 && releaseMoves === 0 ? 'reserve'
          : releaseMoves > 0 && reserveMoves === 0 ? 'release' : 'update')
        : (V.kind === 'maker' && pfPrevRound !== null && currentRound === pfPrevRound ? 'update'
          : V.phase === 'deal' ? 'reserve'
          : V.phase === 'settle' || V.phase === 'done' ? 'settle' : 'update');
    }
    if ((V.phase === 'settle' || V.phase === 'done') && settlement.status) pfRoundBaseline = null;
  }
  pfPrev = snap;
  pfPrevRound = currentRound;
}
function deltaLabel(){
  return pfDeltaKind === 'reserve' ? t('pfDeltaReserve')
    : pfDeltaKind === 'release' ? t('pfDeltaRelease')
    : pfDeltaKind === 'settle' ? t('pfDeltaSettle')
    : pfDeltaPhase === 'deal' ? t('pfDeltaReserve') : t('pfDeltaUpdate');
}
function pfItem(cls, label, available, reserved, total, deltaKey){
  const item = el('div', 'pf-item ' + cls);
  item.appendChild(el('div', 'pf-label', label));
  item.appendChild(el('div', 'pf-value', showCash(available)));
  const sub = el('div', 'pf-sub');
  sub.appendChild(document.createTextNode(t('pfReserved') + ' '));
  sub.appendChild(el('b', null, showCash(reserved)));
  sub.appendChild(document.createTextNode(' · ' + t('pfTotal') + ' ' + showCash(total)));
  item.appendChild(sub);
  const d = pfDelta[deltaKey];
  if (d){ item.appendChild(el('div', 'pf-delta ' + (d > 0 ? 'up' : 'down'), (d > 0 ? '+' : '') + showCash(d) + ' (' + deltaLabel() + ')')); }
  return item;
}
function drawPortfolioStrip(){
  const strip = $('portfolio-strip');
  if (!strip || !V) return;
  strip.textContent = '';
  strip.classList.toggle('hide', !V.seat);
  if (!V.seat) return;
  if (V.kind === 'node'){ strip.appendChild(el('div', 'pf-note', t('pfNodeNote'))); return; }
  if (V.kind === 'observer'){
    const d = V.observer || {};
    strip.appendChild(el('div', 'pf-title', t('pfObserverTitle')));
    const wrap = el('div', 'table-wrap portfolio-table-wrap'); wrap.style.gridColumn = '1 / -1';
    const table = el('table'); const head = el('tr');
    ['', t('pfCash'), t('pfReserved')].concat((V.assets || []).map(a => a.name)).forEach(h => head.appendChild(el('th', null, h)));
    table.appendChild(head);
    const row = (name, p, reserve) => {
      const tr = el('tr'); tr.appendChild(el('td', null, name));
      tr.appendChild(el('td', 'mono', showCash(p.cash.available)));
      tr.appendChild(el('td', 'mono', showCash(p.cash.reserved)));
      (p.inventory || []).forEach(r => tr.appendChild(el('td', 'mono', showCash(r.available) + (r.reserved ? ' (+' + showCash(r.reserved) + ')' : ''))));
      table.appendChild(tr); void reserve;
    };
    if (d.taker_portfolio) row(t('gTaker'), d.taker_portfolio);
    (d.maker_portfolios || []).forEach(m => row(t('gMaker') + ' ' + m.maker, m.portfolio, m.reserve));
    wrap.appendChild(table); strip.appendChild(wrap);
    return;
  }
  const portfolio = V.kind === 'taker' ? (V.taker && V.taker.portfolio) : (V.maker && V.maker.portfolio);
  if (!portfolio){ strip.appendChild(el('div', 'pf-note', '--')); return; }
  updateDeltas(portfolio);
  strip.appendChild(el('div', 'pf-title', t('pfMine')));
  strip.appendChild(pfItem('cash', t('pfCash') + ' · ' + t('pfAvailable'), portfolio.cash.available, portfolio.cash.reserved, portfolio.cash.total, 'cash'));
  (portfolio.inventory || []).forEach(row =>
    strip.appendChild(pfItem('inv', row.name + ' · ' + t('pfAvailable'), row.available, row.reserved, row.total, 'inv' + row.asset)));
}

/* ========================================================================= */
/*  Chat: sentences → the same messages the panel sends                      */
/* ========================================================================= */
function findAsset(text, assets){
  const lower = text.toLowerCase();
  let found = null;
  (assets || []).forEach((a, i) => {
    const aliases = [a.name.toLowerCase()].concat(ASSET_ALIASES[a.name] || []);
    if (aliases.some(alias => lower.includes(alias))) found = i;
  });
  return found;
}
function clampNote(name, value, lo, hi, warnings){
  const v = Math.max(lo, Math.min(hi, value));
  if (v !== value) warnings.push(tf('chatClamped', {name, lo, hi}));
  return v;
}
function firstNumber(text, patterns){
  for (const re of patterns){ const m = re.exec(text); if (m){ const n = m.slice(1).find(g => g !== undefined && /^-?\d/.test(g)); if (n !== undefined) return n; } }
  return null;
}
/* Taker: returns {actions:[{msg, desc:[...]}], warnings:[...]} or null. */
function interpretTaker(text, view){
  const assets = (view && view.assets) || [];
  const pending = (view && view.taker && view.taker.pending) || {asset: 0, qty: 100, direction: 0, is_real: 1, limit_price: 0};
  const values = {}, warnings = [], desc = [];
  const asset = findAsset(text, assets);
  if (asset !== null){ values.asset = asset; }
  const wantsBuy = /買い|買う|買って|買付|購入|買いたい|\bbuy\b/i.test(text);
  const wantsSell = /売り|売る|売って|売却|売りたい|\bsell\b/i.test(text);
  if (wantsBuy && wantsSell) warnings.push(t('chatBothSides'));
  else if (wantsBuy) values.direction = 0;
  else if (wantsSell) values.direction = 1;
  const qtyRaw = firstNumber(text, [
    /(\d+)\s*(?:単位|枚|ユニット|units?|lots?|個)/i,
    /(?:数量|qty|size|amount)\s*(?:を|は|=|:)?\s*(\d+)/i,
    /(\d+)\s*(?:を)?\s*(?:買|売)/
  ]);
  if (qtyRaw !== null) values.qty = clampNote(t('qty'), parseInt(qtyRaw, 10), 1, 500, warnings);
  const limitRaw = firstNumber(text, [
    /(?:上限|下限|指値|価格上限|価格|リミット|limit|price)\s*(?:を|は|=|:)?\s*(\d+(?:\.\d+)?)/i,
    /(\d+(?:\.\d+)?)\s*(?:円|ドル)?\s*(?:以下|以上|まで)/
  ]);
  const effectiveAsset = values.asset !== undefined ? values.asset : pending.asset;
  if (limitRaw !== null){
    const scale = assetScale(assets, effectiveAsset);
    values.limit_price = Math.max(1, Math.round(parseFloat(limitRaw) * scale));
  }
  if (/ダミー|練習|空注文|dummy|cover/i.test(text)) values.is_real = 0;
  else if (/実注文|本番|本当の|本物|\breal\b|\blive\b/i.test(text)) values.is_real = 1;
  const wantsSubmit = /送信|発注|出して|注文して|注文する|実行|お願い|\bsubmit\b|\bsend\b|\bgo\b/i.test(text);
  const merged = Object.assign({}, pending, values);
  if (values.asset !== undefined) desc.push(`${t('asset')}: ${(assets[values.asset] || {}).name}`);
  if (values.direction !== undefined) desc.push(`${t('side')}: ${values.direction ? t('sell') : t('buy')}`);
  if (values.qty !== undefined) desc.push(`${t('qty')}: ${values.qty}`);
  if (values.limit_price !== undefined) desc.push(`${t('limitPrice')}: ${showPrice(assets, effectiveAsset, values.limit_price)}（${merged.direction ? t('limitSell') : t('limitBuy')}）`);
  if (values.is_real !== undefined) desc.push(`${t('kind')}: ${values.is_real ? t('real') : t('cover')}`);
  if (!Object.keys(values).length && !wantsSubmit) return null;
  const actions = [];
  if (Object.keys(values).length) actions.push({msg: {type: 'request', values}, desc});
  if (wantsSubmit){
    const lines = [t('chatSubmitDesc')];
    if (merged.is_real){
      const limit = values.limit_price !== undefined ? values.limit_price : (values.asset !== undefined && values.asset !== pending.asset ? null : pending.limit_price);
      if (merged.direction === 0 && limit) lines.push(tf('chatReserveCash', {amount: showCash(merged.qty * limit), qty: merged.qty, limit: showPrice(assets, merged.asset, limit)}));
      else if (merged.direction === 1) lines.push(tf('chatReserveInv', {qty: merged.qty, asset: (assets[merged.asset] || {}).name}));
    }
    actions.push({msg: {type: 'submit'}, desc: lines});
  }
  return {actions, warnings};
}
/* Maker: returns {actions:[{msg, desc}], warnings} or null. */
function interpretMaker(text, view){
  const assets = (view && view.assets) || [];
  const policy = (view && view.maker && view.maker.policy) || null;
  if (!policy) return null;
  const values = {}, warnings = [], desc = [];
  const asset = findAsset(text, assets);
  if (asset !== null) values.asset = asset;
  const fields = [
    ['spread', /スプレッド|売買の差|売買差|\bspread\b/i],
    ['ask_level', /基準からのずれ|乖離|オフセット|ずれ|\blevel\b|\boffset\b/i],
    ['slope', /数量比例|傾き|\bslope\b|size charge/i],
    ['invcoef', /在庫感応|感応度|\bskew weight\b|\binvcoef\b/i],
    ['inv', /在庫調整|在庫の傾き|inventory skew|\binv\b/i],
    ['maxqty', /最大数量|上限数量|予約枠|最大|max(?:imum)?(?:\s*size|\s*qty)?/i]
  ];
  for (const [name, re] of fields){
    const m = re.exec(text);
    if (!m) continue;
    const after = text.slice(m.index + m[0].length);
    let value = null;
    let rel = /^\s*(?:を|は|=|:)?\s*([+-]?\d+)\s*(?:に|へ|にして|にする|ティック|tick|units?)?\s*(広げ|上げ|増や|大きく|プラス|up|higher|wider|狭め|下げ|減ら|小さく|マイナス|down|lower|narrower)?/.exec(after);
    if (rel && rel[1] !== undefined){
      const n = parseInt(rel[1], 10);
      const word = rel[2] || '';
      if (/広げ|上げ|増や|大きく|プラス|up|higher|wider/.test(word)) value = policy[name] + Math.abs(n);
      else if (/狭め|下げ|減ら|小さく|マイナス|down|lower|narrower/.test(word)) value = policy[name] - Math.abs(n);
      else if (/^[+-]/.test(rel[1])) value = policy[name] + n;
      else value = n;
    }
    if (value === null) continue;
    const [lo, hi] = POLICY_RANGE[name];
    values[name] = clampNote(t(name), value, lo, hi, warnings);
  }
  if (/(停止|止め|休止|やめ|オフ|switch off|turn off|\boff\b)/i.test(text)) values.active = 0;
  else if (/(再開|稼働|開始|オン|switch on|turn on|\bon\b)/i.test(text)) values.active = 1;
  if (!Object.keys(values).length) return null;
  const next = Object.assign({}, policy, values);
  if (values.asset !== undefined) desc.push(`${t('assetLabel')}: ${(assets[policy.asset] || {}).name} → ${(assets[values.asset] || {}).name}`);
  Object.keys(POLICY_RANGE).forEach(name => { if (values[name] !== undefined) desc.push(`${t(name)}: ${policy[name]} → ${values[name]}`); });
  if (values.active !== undefined) desc.push(`${t('active')}: ${policy.active ? t('on') : t('off')} → ${values.active ? t('on') : t('off')}`);
  const oldReserve = (view && view.maker && view.maker.reserve) || {inventory: 0, cash: 0};
  const inventory = next.active ? next.maxqty : 0;
  const cash = makerCashRequirement(next, assets);
  if (inventory !== oldReserve.inventory || cash !== oldReserve.cash)
    desc.push(tf('chatReserveMaker', {inventory: showCash(inventory), cash: showCash(cash), oldInventory: showCash(oldReserve.inventory), oldCash: showCash(oldReserve.cash)}));
  return {actions: [{msg: {type: 'policy', values}, desc}], warnings};
}
function handleChatInput(){
  const input = $('chat-input');
  const text = input.value.trim();
  if (!text || !V) return;
  addChatMessage(text, 'user');
  input.value = '';
  if (chatPending && /^(やめる|取消|取り消し|キャンセル|cancel|no)$/i.test(text)){ cancelChatPreview(); return; }
  if (chatPending && /^(はい|送信する|送信|実行|ok|yes|go)$/i.test(text)){ executeChatActions(); return; }
  const result = V.kind === 'taker' ? interpretTaker(text, V) : V.kind === 'maker' ? interpretMaker(text, V) : null;
  if (!result || !result.actions.length){
    addChatMessage(t('chatUnknown') + t(V.kind === 'taker' ? 'hintsTaker' : 'hintsMaker').slice(0, 3).map(h => '「' + h + '」').join(' '), 'system');
    return;
  }
  result.warnings.forEach(w => addChatMessage(w, 'system'));
  if (V.kind === 'taker' && result.actions.some(a => a.msg.type === 'submit') && isBusy()){
    addChatMessage(t('chatBusy'), 'error');
    result.actions = result.actions.filter(a => a.msg.type !== 'submit');
    if (!result.actions.length) return;
  }
  chatPending = result.actions;
  showChatPreview(result.actions);
}
function showChatPreview(actions){
  const preview = $('chat-preview');
  preview.textContent = '';
  preview.classList.remove('hide');
  preview.appendChild(el('div', 'preview-title', t('chatConfirm')));
  const list = el('ul', 'preview-list');
  actions.forEach(a => a.desc.forEach(line => list.appendChild(el('li', null, line))));
  preview.appendChild(list);
  const tech = el('details', 'preview-tech');
  tech.appendChild(el('summary', null, t('chatTech')));
  tech.appendChild(el('div', 'tag', t('chatRaw')));
  tech.appendChild(el('div', 'preview-raw', actions.map(a => JSON.stringify(a.msg)).join('  ')));
  preview.appendChild(tech);
  const row = el('div', 'preview-actions');
  const yes = el('button', 'go small', t('chatYes')); yes.type = 'button'; yes.onclick = executeChatActions;
  const no = el('button', 'go small ghost', t('chatNo')); no.type = 'button'; no.onclick = cancelChatPreview;
  row.appendChild(yes); row.appendChild(no); preview.appendChild(row);
  yes.focus();
}
function executeChatActions(){
  if (!chatPending) return;
  const submitted = chatPending.some(a => a.msg.type === 'submit');
  const sent = chatPending.every(a => send(a.msg));
  addChatMessage(sent ? t(submitted ? 'chatSubmitted' : 'chatApplied') : t('reconnecting'), sent ? 'ok' : 'error');
  chatAwaitingReply = sent;
  setTimeout(() => { chatAwaitingReply = false; }, 4000);
  chatPending = null;
  $('chat-preview').classList.add('hide');
  $('chat-input').focus();
}
function cancelChatPreview(){
  chatPending = null;
  $('chat-preview').classList.add('hide');
  addChatMessage(t('chatCancelled'), 'system');
  $('chat-input').focus();
}
function addChatMessage(text, who){
  chatHistory.push({text, who});
  if (chatHistory.length > 60) chatHistory.shift();
  renderChatMessages();
}
function renderChatMessages(){
  const box = $('chat-messages');
  if (!box) return;
  box.textContent = '';
  chatHistory.forEach(m => box.appendChild(el('div', 'chat-msg ' + m.who, m.text)));
  box.scrollTop = box.scrollHeight;
}
function drawChat(){
  const panel = $('chat-panel');
  const chatty = V.kind === 'taker' || V.kind === 'maker';
  panel.classList.toggle('hide', !chatty);
  if (!chatty) return;
  $('chat-why').textContent = t(V.kind === 'taker' ? 'chatWhyTaker' : 'chatWhyMaker');
  $('chat-input').placeholder = t(V.kind === 'taker' ? 'chatPlaceholderTaker' : 'chatPlaceholderMaker');
  if (!chatHistory.length) addChatMessage(t(V.kind === 'taker' ? 'chatWelcomeTaker' : 'chatWelcomeMaker'), 'system');
  const hints = $('chat-hints');
  const want = V.kind + ':' + lang;
  if (hints.dataset.for !== want){
    hints.dataset.for = want; hints.textContent = '';
    t(V.kind === 'taker' ? 'hintsTaker' : 'hintsMaker').forEach(h => {
      const b = el('button', 'hint-chip', h); b.type = 'button';
      b.onclick = () => { $('chat-input').value = h; handleChatInput(); };
      hints.appendChild(b);
    });
  }
}

/* ========================================================================= */
/*  Render frame                                                              */
/* ========================================================================= */
function render(){
  if (!V) return;
  $('countdown').textContent =
    (V.config.auto_rounds && V.next_round_in !== null && V.next_round_in !== undefined)
      ? '⟳ ' + V.next_round_in.toFixed(0) + 's' : '';
  const signature = JSON.stringify(V, (k, v) => k === 'next_round_in' ? 0 : v);
  if (signature === lastDraw) return;
  lastDraw = signature;
  document.documentElement.lang = lang;
  document.querySelectorAll('[data-s]').forEach(n => {
    const v = t(n.dataset.s); if (typeof v === 'string') n.textContent = v; });
  document.querySelectorAll('[data-s-placeholder]').forEach(n => {
    const v = t(n.dataset.sPlaceholder); if (typeof v === 'string') n.placeholder = v; });
  $('btn-lang').textContent = t('langOther');
  const c = V.config;
  const eng = $('engineBadge');
  eng.textContent = c.engine === 'sim' ? t('engineSim') : t('engineMpc');
  eng.dataset.short = c.engine === 'sim' ? t('engineSimShort') : t('engineMpcShort');
  eng.title = (c.engine === 'sim' ? t('engineSimNote') : t('engineMpcNote'))
    + (lang === 'en' && c.engine_note ? ' · ' + c.engine_note : '');
  eng.className = 'badge engine' + (c.engine === 'sim' ? '' : ' mpc');
  $('roundNum').textContent = (V.public && V.public.number)
    ? '#' + V.public.number + ' · ' + V.public.ms + ' ms' : '#--';
  const badge = $('seatBadge');
  const seatName = V.kind === 'taker' ? t('seatTaker') : V.kind === 'maker' ? t('seatMaker') + ' ' + V.index
    : V.kind === 'node' ? t('seatNode') + ' ' + V.index : V.kind === 'observer' ? t('seatObserver') : '';
  badge.textContent = seatName + (V.label ? ' · ' + V.label : '');
  badge.classList.toggle('hide', !V.seat);
  $('btn-leave').classList.toggle('hide', !V.seat);
  $('lobby').classList.toggle('hide', !!V.seat);
  $('stage').classList.toggle('hide', !V.seat);
  if (!V.seat){ buildLobby(); return; }
  drawPhaseStrip();
  drawNetworkGraph();
  drawPortfolioStrip();
  drawChat();
  const want = V.kind + ':' + V.index + ':' + lang;
  if (built !== want){ buildPanel(); built = want; }
  updatePanel();
  drawPublic();
  drawHistory();
  drawSeatMap();
  drawNotices();
}

function drawPublic(){
  const p = V.public || {}, box = $('publicbody');
  box.textContent = '';
  if (!p.number){ box.appendChild(el('p', 'empty', t('noRoundYet'))); return; }
  const key = el('div', 'leak-box');
  key.appendChild(el('div', 'tag leak', t('opened') + ' · ' + t('everyoneSees')));
  key.appendChild(el('div', 'opaque', p.masked_key));
  box.appendChild(key);
  const table = el('table');
  const rows = [
    [t('corrected'), p.corrections + ' / ' + p.reductions],
    [t('capacityProduct'), p.product_capacity],
    [t('capacityOpen'), p.open_capacity],
  ];
  if ((p.named || []).length) rows.push([t('nNamed'), nodeList(p.named)]);
  if ((p.silent || []).length) rows.push([t('silent'), nodeList(p.silent)]);
  const st = p.engine_stats || {};
  if (st.protocol_ms !== undefined){
    rows.length = 0;
    rows.push([t('protocolMs'), Number(st.protocol_ms).toFixed(1) + ' ms'],
              [t('engRounds'), st.rounds], [t('engMb'), st.mb],
              [t('compiledOnce'), st.compiled_once_ms + ' ms']);
  }
  rows.forEach(([a, b]) => { const tr = el('tr');
    tr.appendChild(el('td', null, a)); tr.appendChild(el('td', 'mono', String(b)));
    table.appendChild(tr); });
  box.appendChild(table);
  if (p.settlement){
    const settlement = el('div', 'hidden-box stack'); settlement.style.marginTop = '.5rem';
    settlement.appendChild(el('div', 'tag', t('gLedger')));
    settlement.appendChild(el('span', 'status-pill ' + p.settlement.status, settlementName(p.settlement.status)));
    settlement.appendChild(el('div', 'root', p.settlement.state_root || '--'));
    box.appendChild(settlement);
  }
  if (p.verified !== null && p.verified !== undefined){
    const v = el('div', p.verified ? 'hidden-box' : 'bad-box');
    v.style.marginTop = '.5rem';
    v.appendChild(el('div', p.verified ? 'tag' : 'tag bad', p.verified ? t('verifiedYes') : t('verifiedNo')));
    v.appendChild(el('div', 'why mono', p.verified_detail));
    box.appendChild(v);
  }
  if (p.aborted){ const bad = el('div', 'bad-box');
    bad.style.marginTop = '.5rem';
    bad.appendChild(el('div', 'tag bad', t('stopped')));
    bad.appendChild(el('div', null, abortText(p)));
    bad.appendChild(el('div', 'why', p.abort_reason));
    box.appendChild(bad); }
}
function drawHistory(){
  const box = $('history'); box.textContent = '';
  const rows = (V.history || []).slice().reverse();
  if (!rows.length){ box.appendChild(el('p', 'empty', t('noRoundYet'))); return; }
  rows.forEach(h => {
    const row = el('div', 'history-row');
    row.appendChild(el('span', 'n', '#' + h.number));
    if (h.aborted) row.appendChild(el('span', 'bad', t('stopped') + ' — ' + abortText(h)));
    else if (h.corrections > 0) row.appendChild(el('span', 'warn', tf('n_corrected', {number: h.number, corrections: h.corrections, reductions: h.reductions, named: h.named}).replace(/^#\d+\s*/, '')));
    else row.appendChild(el('span', null, t('phaseDone')));
    if (h.settlement) row.appendChild(el('span', 'status-pill ' + h.settlement.status, settlementName(h.settlement.status)));
    box.appendChild(row);
  });
}
function drawSeatMap(){
  const box = $('seatmap'); box.textContent = '';
  const named = new Set((V.public && V.public.named) || []);
  const silent = new Set((V.public && V.public.silent) || []);
  V.seats.forEach(s => {
    let cls = 'chip' + (s.mode === 'manual' ? ' manual' : '') + (s.mine ? ' mine' : '');
    if (s.kind === 'node' && named.has(s.index)) cls += ' named';
    if (s.kind === 'node' && silent.has(s.index)) cls += ' silent';
    const name = s.kind === 'taker' ? t('seatTaker') : (s.kind === 'maker' ? t('seatMaker') : t('seatNode')) + ' ' + s.index;
    box.appendChild(el('span', cls, name + (s.label ? ' · ' + s.label : '')));
  });
}
function drawNotices(){
  const box = $('notices'); box.textContent = '';
  const rows = (V.notices || []).slice().reverse();
  if (!rows.length){ box.appendChild(el('p', 'empty', '--')); return; }
  rows.forEach(n => box.appendChild(el('div', n.tone, msg(n.code, n.fields))));
}

/* ─── lobby ────────────────────────────────────────────────────────── */
function buildLobby(){
  const grid = $('lobby-grid'); grid.textContent = ''; grid.setAttribute('aria-busy', 'false');
  V.seats.forEach(s => {
    const b = el('button'); b.type = 'button';
    const name = s.kind === 'taker' ? t('seatTaker') : (s.kind === 'maker' ? t('seatMaker') : t('seatNode')) + ' ' + s.index;
    b.appendChild(el('b', null, name));
    b.appendChild(el('span', null, s.held ? (s.label || t('taken')) : t('auto')));
    b.disabled = s.held;
    b.onclick = () => send({type:'claim', seat:s.id, label:$('pname').value});
    grid.appendChild(b);
  });
  const ex = $('explain'); ex.textContent = '';
  t('explain').forEach(([name, why]) => {
    const d = el('div'); d.style.marginBottom = '.4rem';
    d.appendChild(el('b', null, name + ' — '));
    d.appendChild(document.createTextNode(why)); ex.appendChild(d); });
}

/* ─── panels ──────────────────────────────────────────────────────── */
function card(title, why){
  const c = el('div', 'card');
  if (title) c.appendChild(el('h2', null, title));
  if (why) c.appendChild(el('p', 'why', why));
  return c;
}
function slider(parent, label, id, min, max, onInput){
  const row = el('div', 'row');
  const caption = el('label', null, label); caption.htmlFor = id; row.appendChild(caption);
  const i = document.createElement('input');
  i.type = 'range'; i.min = min; i.max = max; i.id = id;
  const v = el('span', 'val', '');
  i.oninput = () => { v.textContent = i.value; onInput(Number(i.value)); };
  row.appendChild(i); row.appendChild(v); parent.appendChild(row);
  return i;
}
function segmented(parent, label, options, onPick, id){
  const row = el('div', 'row');
  if (label) row.appendChild(el('label', null, label));
  const seg = el('div', 'seg'); seg.id = id; seg.setAttribute('role', 'group');
  if (label) seg.setAttribute('aria-label', label);
  options.forEach(o => { const b = el('button', o.tone || '', o.text);
    b.type = 'button'; b.dataset.value = o.value;
    b.setAttribute('aria-pressed', 'false');
    b.onclick = () => onPick(o.value); seg.appendChild(b); });
  row.appendChild(seg); parent.appendChild(row);
  return seg;
}
function pick(seg, value){
  if (!seg) return;
  [...seg.children].forEach(b => { const on = b.dataset.value === String(value);
    b.classList.toggle('on', on); b.setAttribute('aria-pressed', on ? 'true' : 'false'); });
}
function buildPanel(){
  const p = $('panel'); p.textContent = '';
  ({taker: buildTaker, maker: buildMaker, node: buildNode,
    observer: buildObserver}[V.kind] || (() => {}))(p);
}
function updatePanel(){
  ({taker: updateTaker, maker: updateMaker, node: updateNode,
    observer: updateObserver}[V.kind] || (() => {}))();
}

/* ─── taker ────────────────────────────────────────────────────────── */
let takerEls = {};
function buildTaker(root){
  const c = card(t('takerTitle'), t('takerWhy'));
  const assetRow = el('div', 'row');
  const assetLabel = el('label', null, t('asset')); assetLabel.htmlFor = 'taker_asset';
  assetRow.appendChild(assetLabel);
  const sel = document.createElement('select'); sel.id = 'taker_asset';
  V.assets.forEach((a, i) => { const o = document.createElement('option');
    o.value = i; o.textContent = a.name + '  ' + showPrice(V.assets, i, a.reference); sel.appendChild(o); });
  sel.onchange = () => send({type:'request', values:{asset:Number(sel.value)}});
  assetRow.appendChild(sel); c.appendChild(assetRow);
  const side = segmented(c, t('side'), [{value:0, text:t('buy')}, {value:1, text:t('sell')}],
    (v) => send({type:'request', values:{direction:v}}), 'taker_side');
  const qty = slider(c, t('qty'), 'taker_qty', 1, 500, (v) => send({type:'request', values:{qty:v}}));
  const limitRow = el('div', 'row');
  const limitLabel = el('label', null, t('limitPrice')); limitLabel.htmlFor = 'taker_limit';
  limitRow.appendChild(limitLabel);
  const limit = document.createElement('input'); limit.type = 'number'; limit.id = 'taker_limit';
  limit.min = '0'; limit.step = '0.01';
  limit.onchange = () => {
    const pending = (V.taker && V.taker.pending) || {asset:0};
    const scale = assetScale(V.assets, pending.asset);
    send({type:'request', values:{limit_price:Math.max(1, Math.round(Number(limit.value) * scale))}});
  };
  const limitHint = el('span', 'hint', '');
  limitRow.appendChild(limit); limitRow.appendChild(limitHint); c.appendChild(limitRow);
  const kind = segmented(c, t('kind'), [{value:1, text:t('real'), tone:'amber'}, {value:0, text:t('cover'), tone:'teal'}],
    (v) => send({type:'request', values:{is_real:v}}), 'taker_kind');
  const reserve = el('div', 'note-box'); c.appendChild(reserve);
  c.appendChild(el('p', 'why', t('coverWhy')));
  c.appendChild(el('div', 'hidden-box', t('automaticSettle')));
  const go = el('button', 'go', t('submit')); go.type = 'button';
  go.style.marginTop = '.7rem';
  go.onclick = () => { go.disabled = true; go.textContent = t('waiting'); send({type:'submit'}); };
  c.appendChild(go);
  root.appendChild(c);

  const r = card(t('openedTitle'), t('openedWhy'));
  const opened = el('div', 'leak-box');
  opened.appendChild(el('div', 'tag leak', t('everyoneSees')));
  const maskedKey = el('div', 'opaque', '--');
  opened.appendChild(maskedKey); r.appendChild(opened);
  r.appendChild(el('div', 'why', '↓ ' + t('minusMask')));
  const priv = el('div', 'hidden-box');
  priv.appendChild(el('div', 'tag', t('yourPrice')));
  const price = el('div', 'big', '--'); priv.appendChild(price);
  const winner = el('div', null, ''); priv.appendChild(winner);
  r.appendChild(priv);
  root.appendChild(r);
  const settlementCard = card(t('settlementTitle'), '');
  const settlement = el('div'); settlementCard.appendChild(settlement); root.appendChild(settlementCard);
  takerEls = {sel, side, qty, limitLabel, limit, limitHint, kind, reserve, go, maskedKey, price, winner, settlement};
}
function updateTaker(){
  const d = V.taker || {}, p = d.pending || {}, last = d.last, e = takerEls;
  setVal(e.sel, p.asset); pick(e.side, p.direction); pick(e.kind, p.is_real);
  setVal(e.qty, p.qty);
  if (e.qty && e.qty.parentElement) e.qty.parentElement.querySelector('.val').textContent = p.qty;
  const asset = V.assets[p.asset] || {scale:1};
  const digits = String(asset.scale).length - 1;
  setVal(e.limit, (Number(p.limit_price || 0) / asset.scale).toFixed(digits));
  e.limitLabel.textContent = p.direction === 0 ? t('limitPriceBuy') : t('limitPriceSell');
  e.limitHint.textContent = p.direction === 0 ? t('limitBuy') : t('limitSell');
  e.reserve.textContent = t('reserveOnSend') + ': ' + (p.is_real
    ? (p.direction === 0 ? tf('chatReserveCash', {amount: showCash(p.qty * p.limit_price), qty: p.qty, limit: showPrice(V.assets, p.asset, p.limit_price)})
                         : tf('chatReserveInv', {qty: p.qty, asset: asset.name}))
    : t('coverStatus'));
  const busy = isBusy();
  e.go.disabled = busy;
  e.go.textContent = busy ? t('waiting') : t('submit');
  drawSettlement(e.settlement, d.settlement);
  if (!last){ e.maskedKey.textContent = '--'; e.price.textContent = '--'; e.winner.textContent = t('noRoundYet'); return; }
  e.maskedKey.textContent = (V.public && V.public.masked_key) || '--';
  if (V.public && V.public.aborted){
    e.price.textContent = '--'; e.winner.textContent = t('stopped') + ' — ' + abortText(V.public);
    return;
  }
  if (last.winner === null || last.winner === undefined){
    e.price.textContent = '--'; e.winner.textContent = t('noMaker');
    return;
  }
  e.price.textContent = showPrice(V.assets, last.asset, last.price);
  e.winner.textContent = t('winnerIs') + ': ' + t('gMaker') + ' ' + last.winner
    + '   ·   ' + t('eligible') + ' ' + last.eligible
    + (last.is_real ? '' : '   ·   ' + t('coverRound'));
}

/* ─── maker ────────────────────────────────────────────────────────── */
let makerEls = {};
function buildMaker(root){
  const c = card(t('makerTitle'), t('makerWhy'));
  const assetRow = el('div', 'row');
  const assetLabel = el('label', null, t('assetLabel')); assetLabel.htmlFor = 'maker_asset';
  assetRow.appendChild(assetLabel);
  const sel = document.createElement('select'); sel.id = 'maker_asset';
  V.assets.forEach((a, i) => { const o = document.createElement('option');
    o.value = i; o.textContent = a.name; sel.appendChild(o); });
  sel.onchange = () => send({type:'policy', values:{asset:Number(sel.value)}});
  assetRow.appendChild(sel); c.appendChild(assetRow);
  const sliders = {};
  Object.keys(POLICY_RANGE).forEach(name => {
    const [lo, hi] = POLICY_RANGE[name];
    sliders[name] = slider(c, t(name), 'p_' + name, lo, hi,
      (v) => { const values = {}; values[name] = v; send({type:'policy', values}); });
  });
  c.appendChild(el('p', 'why', t('invWhy')));
  const act = segmented(c, t('active'), [{value:1, text:t('on')}, {value:0, text:t('off')}],
    (v) => send({type:'policy', values:{active:v}}), 'maker_active');
  root.appendChild(c);

  const reserveCard = card(t('reserveTitle'), t('reserveWhy'));
  const reserve = el('div'); reserveCard.appendChild(reserve); root.appendChild(reserveCard);

  const f = card(t('fillTitle'), '');
  const fill = el('div'); f.appendChild(fill); root.appendChild(f);

  const settlementCard = card(t('settlementTitle'), '');
  const settlement = el('div'); settlementCard.appendChild(settlement); root.appendChild(settlementCard);

  const local = card(t('tryTitle'), t('tryWhy'));
  const row = el('div', 'row');
  const localQtyLabel = el('label', null, t('tryQty')); localQtyLabel.htmlFor = 'try_qty';
  row.appendChild(localQtyLabel);
  const q = document.createElement('input');
  q.type = 'number'; q.id = 'try_qty'; q.value = 100; q.min = 1; q.max = 500;
  row.appendChild(q); local.appendChild(row);
  const out = el('table'); local.appendChild(out);
  q.oninput = () => drawLocal(out, Number(q.value));
  root.appendChild(local);
  makerEls = {sel, sliders, act, reserve, fill, settlement, out, q};
}
function drawLocal(out, qty){
  const pol = (V.maker && V.maker.policy) || null;
  out.textContent = '';
  if (!pol) return;
  const ref = V.assets[pol.asset] ? V.assets[pol.asset].reference : 0;
  const anchor = pol.use_ref * ref + pol.ask_level;
  const depth = pol.slope * qty, skew = pol.invcoef * pol.inv;
  [[t('ask'), anchor + depth + skew], [t('bid'), anchor - pol.spread - depth + skew]].forEach(([k, v]) => {
    const tr = el('tr'); tr.appendChild(el('td', null, k));
    tr.appendChild(el('td', 'mono', showPrice(V.assets, pol.asset, v)));
    out.appendChild(tr); });
  const warn = qty > pol.maxqty || !pol.active;
  const tr = el('tr'); tr.appendChild(el('td', null, ''));
  tr.appendChild(el('td', null, warn ? (pol.active ? '> ' + t('maxqty') : t('off')) : ''));
  out.appendChild(tr);
}
function updateMaker(){
  const d = V.maker || {}, pol = d.policy, e = makerEls;
  if (!pol) return;
  setVal(e.sel, pol.asset);
  Object.keys(e.sliders).forEach(name => {
    setVal(e.sliders[name], pol[name]);
    e.sliders[name].parentElement.querySelector('.val').textContent = pol[name]; });
  pick(e.act, pol.active);
  e.reserve.textContent = '';
  if (d.reserve){
    const grid = el('div', 'balance-grid');
    grid.appendChild(metric(t('assetLabel'), (V.assets[d.reserve.asset] || {}).name || '--'));
    grid.appendChild(metric(t('inventoryReserve'), showCash(d.reserve.inventory)));
    grid.appendChild(metric(t('cashReserve'), showCash(d.reserve.cash)));
    e.reserve.appendChild(grid);
  } else e.reserve.appendChild(el('p', 'empty', '--'));
  e.fill.textContent = '';
  if (d.fill){
    const box = el('div', 'hidden-box');
    box.appendChild(el('div', 'tag', t('yourFill')));
    box.appendChild(el('div', 'big', showPrice(V.assets, d.fill.asset, d.fill.price)));
    box.appendChild(el('div', null, V.assets[d.fill.asset].name + '  ' + (d.fill.direction ? t('buy') : t('sell')) + '  ' + d.fill.qty));
    e.fill.appendChild(box);
  } else {
    e.fill.appendChild(el('p', 'empty', t('noFill')));
    e.fill.appendChild(el('div', 'note-box', t('noFillWhy')));
  }
  drawSettlement(e.settlement, d.settlement);
  drawLocal(e.out, Number(e.q.value));
}

/* ─── node ─────────────────────────────────────────────────────────── */
let nodeEls = {};
function buildNode(root){
  const c = card(t('behaviourTitle'), t('behaviourWhy'));
  const inert = el('div', 'note-box hide');
  inert.appendChild(el('b', null, t('inertTitle')));
  inert.appendChild(el('div', null, t('inertWhy')));
  c.appendChild(inert);
  const list = el('div', 'behaviour-list'); list.setAttribute('role', 'radiogroup');
  list.setAttribute('aria-label', t('behaviourTitle'));
  const buttons = {};
  BEHAVIOURS.forEach(name => {
    const b = el('button', 'behaviour-btn'); b.type = 'button'; b.setAttribute('role', 'radio');
    b.appendChild(el('b', null, t('b_' + name)));
    b.appendChild(el('div', 'why', t('b_' + name + '_d')));
    b.onclick = () => send({type:'behaviour', value:name});
    buttons[name] = b; list.appendChild(b);
  });
  c.appendChild(list); root.appendChild(c);

  const s = card(t('nodeTitle'), t('nodeWhy'));
  s.appendChild(el('div', 'hidden-box', t('noCustody')));
  const shares = el('div'); shares.style.marginTop = '.5rem'; s.appendChild(shares); root.appendChild(s);

  const v = card(t('verdictTitle'), '');
  const verdict = el('div'); v.appendChild(verdict); root.appendChild(v);
  nodeEls = {buttons, shares, verdict, inert};
}
function updateNode(){
  const d = V.node || {}, p = V.public || {}, e = nodeEls;
  const inert = V.config.engine !== 'sim' && !V.config.robust;
  e.inert.classList.toggle('hide', !inert);
  BEHAVIOURS.forEach(name => {
    const on = d.behaviour === name;
    e.buttons[name].classList.toggle('on', on);
    e.buttons[name].setAttribute('aria-checked', on ? 'true' : 'false');
    e.buttons[name].disabled = inert && name !== 'honest';
  });
  e.shares.textContent = '';
  (d.shares || []).forEach(h => e.shares.appendChild(el('div', 'opaque', h)));
  if (!(d.shares || []).length) e.shares.appendChild(el('p', 'empty', t('noRoundYet')));

  e.verdict.textContent = '';
  if (!p.number){ e.verdict.appendChild(el('p', 'empty', t('noRoundYet'))); return; }
  if (d.rejected_me){
    const b = el('div', 'bad-box'); b.appendChild(el('div', 'tag bad', t('refused')));
    b.appendChild(el('div', null, t('rejectedYou'))); e.verdict.appendChild(b);
  } else if (d.named_me){
    const b = el('div', 'bad-box'); b.appendChild(el('div', 'tag bad', t('namedYou')));
    b.appendChild(el('div', 'big', d.times_named + ' / ' + p.reductions));
    e.verdict.appendChild(b);
  } else {
    const b = el('div', 'hidden-box');
    b.appendChild(el('div', 'tag', p.named && p.named.length ? nodeList(p.named) : t('namedNobody')));
    b.appendChild(el('div', 'big', (p.corrections || 0) + ' / ' + (p.reductions || 0)));
    b.appendChild(el('div', 'why', t('corrected')));
    e.verdict.appendChild(b);
  }
  if (p.aborted){ const bad = el('div', 'bad-box'); bad.style.marginTop = '.5rem';
    bad.appendChild(el('div', null, abortText(p)));
    bad.appendChild(el('div', 'why', p.abort_reason));
    e.verdict.appendChild(bad); }
}

/* ─── observer ─────────────────────────────────────────────────────── */
let obsEls = {};
function buildObserver(root){
  const banner = el('div', 'note-box', t('observerWhy'));
  banner.style.marginBottom = '.8rem'; root.appendChild(banner);

  const c = card(t('allQuotes'), '');
  const head = el('div', 'row'); c.appendChild(head);
  const wrap = el('div', 'table-wrap'); const table = el('table'); wrap.appendChild(table); c.appendChild(wrap); root.appendChild(c);

  const ledgerCard = card(t('gLedger'), '');
  const ledger = el('div'); ledgerCard.appendChild(ledger); root.appendChild(ledgerCard);

  const cfg = card(t('settings'), '');
  const rs = slider(cfg, t('roundEvery'), 'cfg_rs', 2, 60, (v) => send({type:'config', values:{round_seconds:v}}));
  const sm = slider(cfg, t('stepMs'), 'cfg_sm', 0, 2000, (v) => send({type:'config', values:{step_ms:v}}));
  const ar = segmented(cfg, t('autoRounds'), [{value:1, text:t('on')}, {value:0, text:t('off')}],
    (v) => send({type:'config', values:{auto_rounds:!!v}}), 'cfg_ar');
  const ic = segmented(cfg, t('inputCheck'), [{value:1, text:t('on')}, {value:0, text:t('off')}],
    (v) => send({type:'config', values:{input_check:!!v}}), 'cfg_ic');
  cfg.appendChild(el('p', 'why', t('inputCheckWhy')));
  const now = el('button', 'go', t('runNow')); now.type = 'button';
  now.style.marginTop = '.6rem';
  now.onclick = () => send({type:'submit_any'});
  cfg.appendChild(now);
  root.appendChild(cfg);

  const b = card(t('behaviours'), '');
  const behav = el('div', 'seatmap'); b.appendChild(behav); root.appendChild(b);
  obsEls = {head, table, ledger, rs, sm, ar, ic, now, behav};
}
function updateObserver(){
  const d = V.observer || {}, e = obsEls;
  e.head.textContent = '';
  if (d.request){
    const parts = [
      [t('request'), V.assets[d.request.asset].name + ' ' + (d.request.direction ? t('sell') : t('buy')) + ' ' + d.request.qty + (d.request.is_real ? '' : ' · ' + t('cover'))],
      [t('winner'), d.winner === null || d.winner === undefined ? '--' : t('gMaker') + ' ' + d.winner],
      [t('price'), showPrice(V.assets, d.request.asset, d.price)],
    ];
    parts.forEach(([k, v]) => { const box = el('div', 'hidden-box');
      box.style.flex = '1'; box.appendChild(el('div', 'tag', k));
      box.appendChild(el('div', null, v)); e.head.appendChild(box); });
  } else e.head.appendChild(el('p', 'empty', t('noRoundYet')));
  e.table.textContent = '';
  const hr = el('tr');
  [t('maker'), t('assetLabel'), t('ask'), t('bid'), t('reason')].forEach(h => hr.appendChild(el('th', null, h)));
  e.table.appendChild(hr);
  // Every quote is anchored on the market that was asked for, so that is
  // the scale they are all shown at.
  const shownIn = d.request ? d.request.asset : 0;
  (d.quotes || []).forEach(q => {
    const tr = el('tr', q.maker === d.winner ? 'win' : (q.eligible ? '' : 'out'));
    tr.appendChild(el('td', null, t('gMaker') + ' ' + q.maker));
    tr.appendChild(el('td', null, (V.assets[q.asset] || {}).name || '--'));
    tr.appendChild(el('td', 'mono', showPrice(V.assets, shownIn, q.ask)));
    tr.appendChild(el('td', 'mono', showPrice(V.assets, shownIn, q.bid)));
    tr.appendChild(el('td', null, showQuoteReason(q.reason)));
    e.table.appendChild(tr);
  });
  e.ledger.textContent = '';
  const latest = (d.settlements || [])[0];
  if (latest) drawSettlement(e.ledger, latest); else e.ledger.appendChild(el('p', 'empty', t('noSettlement')));
  const c = V.config;
  setVal(e.rs, c.round_seconds);
  e.rs.parentElement.querySelector('.val').textContent = c.round_seconds;
  setVal(e.sm, c.step_ms);
  e.sm.parentElement.querySelector('.val').textContent = c.step_ms;
  pick(e.ar, c.auto_rounds ? 1 : 0);
  pick(e.ic, c.input_check ? 1 : 0);
  e.now.disabled = isBusy();
  e.behav.textContent = '';
  Object.keys(d.behaviours || {}).forEach(j => {
    const b = d.behaviours[j];
    e.behav.appendChild(el('span', 'chip' + (b === 'honest' ? '' : ' named'), t('gNode') + ' ' + j + ' · ' + t('b_' + b)));
  });
}

/* ─── boot ─────────────────────────────────────────────────────────── */
if (hasDom){
  $('btn-lang').onclick = () => { lang = lang === 'ja' ? 'en' : 'ja';
    try { localStorage.setItem('qomm.lang', lang); } catch (e) { /* private mode */ }
    built = ''; lastDraw = ''; chatHistory = []; chatPending = null; $('chat-preview').classList.add('hide');
    document.querySelectorAll('[data-s]').forEach(n => { const v = t(n.dataset.s); if (typeof v === 'string') n.textContent = v; });
    $('btn-lang').textContent = t('langOther');
    render(); };
  $('btn-leave').onclick = () => { send({type:'release'}); built = ''; chatHistory = []; chatPending = null; pfPrev = null; pfPrevRound = null; pfRoundBaseline = null; pfDelta = {}; pfDeltaKind = ''; };
  $('btn-watch').onclick = () => send({type:'claim', seat:'observer', label:$('pname').value});
  $('chat-input').addEventListener('keydown', e => { if (e.key === 'Enter'){ e.preventDefault(); handleChatInput(); } });
  $('chat-send').addEventListener('click', handleChatInput);
  let resizeTimer = null;
  const redrawIfWidthChanged = () => { clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      const graph = $('network-graph');
      if (V && V.seat && graph && graph.clientWidth !== lastGraphWidth) drawNetworkGraph();
    }, 120); };
  window.addEventListener('resize', redrawIfWidthChanged);
  // The diagram is laid out for the width it had when the first view arrived;
  // if the box changes width afterwards (stylesheet, fonts, orientation), it is
  // laid out again.  Height changes are the diagram's own and do not redraw.
  if (typeof ResizeObserver !== 'undefined') new ResizeObserver(redrawIfWidthChanged).observe($('network-graph'));
  document.querySelectorAll('[data-s]').forEach(n => { const v = t(n.dataset.s); if (typeof v === 'string') n.textContent = v; });
  $('btn-lang').textContent = t('langOther');
  connect();
}
if (typeof module !== 'undefined' && module.exports){
  module.exports = {interpretTaker, interpretMaker, makerCashRequirement, translateRefusal, orthoPath,
    setLang: (l) => { lang = l; }, S};
}
