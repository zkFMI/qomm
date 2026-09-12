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
  title:"QOMM", subtitle:"注文の中身を伏せたまま、いちばん良い気配を出した相手と約定させる仕組みのデモです。",
  engineSim:"試作エンジン（比較は平文）", engineSimShort:"試作", engineSimNote:"値の分割は実装どおりですが、価格の比較は暗号化せずに行います。",
  engineMpc:"秘密計算で価格照合", engineMpcShort:"秘密計算", engineMpcNote:"価格の比較を秘密計算（MP-SPDZ）で行い、検証済みの zkPI を DeFMI へ渡します。",
  leave:"離席", langOther:"EN",
  connecting:"サーバーに接続しています…",
  reconnecting:"接続が切れました。再接続しています…",
  // top bar, guidance, and the drawer that opens from a card in the diagram
  viewLabel:"視点", viewPublic:"未参加（公開情報のみ）", viewHelp:"席を選ぶ・替える",
  myControls:"操作", close:"閉じる", more:"詳しく", less:"たたむ",
  selectHint:"Enter で開きます",
  hintSeatless:"図の席（注文者・値付け・計算ノード）を選ぶと参加できます。処理の札を選ぶと公開情報が見られます。",
  hintSeated:"枠の付いた札があなたの席です。札を選ぶと操作できます。",
  hintObserver:"全体表示はデモ専用の全情報です。札を選ぶと各席の中身が見られ、上の「設定」で部屋を操作できます。",
  hintKeys:"Tab で札を移動、Enter で開く、Esc で閉じる。",
  paneJoinKicker:"参加", paneSwitchTitle:"席を替える・参加状況",
  paneSwitchWhy:"別の席を選ぶと、今の席を離れてその席に移ります。",
  paneMine:"あなたの席", paneProcess:"処理",
  seatAuto:"この席はいま自動で動いています。",
  seatHeldBy:(f)=>`${f.who} が操作中です。`,
  seatPrivate:"この席に届く情報（注文・方針・分割した値）は、席に着いた人の画面にだけ送られます。",
  claimHere:"この席で参加する", claimSwitch:"この席に移る",
  observerSeatWhy:"実運用では、この席の中身は他の席から見えません。まとめて見ているのは全体表示だけです。",
  obsPolicy:"登録済みの方針", obsQuote:"この席の気配", obsBehaviour:"動作設定", obsBalances:"在庫と資金",
  eligibleShort:"条件を満たす",
  paneMatcherWhy:"全社の気配と注文を照合し、価格条件に合ういちばん良い相手を選びます。",
  paneMatcherWhySim:"全社の気配と注文を照合し、価格条件に合ういちばん良い相手を選びます。この試作エンジンでは比較は平文で行います。",
  paneZkpiWhy:"照合の結果が正しく計算されたことを示す証明を作り、台帳側がそれを確かめます。",
  paneLedgerWhy:"検証に合格した取引だけを、予約済みの在庫と資金の同時受け渡しとして記録します。",
  paneObserverTitle:"設定と全参加者の状況",
  techNotes:"技術的な補足", publicRecord:"台帳の記録",
  zkpiTech:"設計では、zkPI は秘密計算の結果が正しい入力から計算されたことを、入力を明かさずに示す証明です。台帳はこれを確かめてから予約済みの資産を動かします。この部屋での実際の記録先は下の注記のとおりです。",
  legendHeading:"凡例", legendMore:"注記と技術的な制約",
  lobbyTitle:"席を選んで参加する",
  lobbyWhy:"席ごとに見える情報が違います。それがこの仕組みの要点です。空いている席は自動で動いています。",
  yourName:"表示名（任意）", watch:"全体表示で見る（デモ専用・全席の情報）",
  theSeats:"空席は自動で動き、名前の付いた席は人が操作しています",
  roleTaker:"注文を出す側（Taker）", roleTakerWhy:"取引したい注文を出します。約定価格を受け取るのはこの席だけです。",
  roleMaker:"気配を出す側（Maker）", roleMakerWhy:"売り買いの気配と最大数量を先に登録し、注文の相手になります。注文の中身は見えません。",
  roleNode:"計算ノード", roleNodeWhy:"分割された値だけで計算します。在庫も資金も持ちません。",
  roleNodeWhySim:"設計では分割された値だけで計算します。この試作エンジンでは、価格の比較は部屋（サーバー）が平文で行います。在庫も資金も持ちません。",
  // Shown wherever the design is described while the room runs the
  // simulation engine: the share layer is real, the comparison is not hidden.
  engineNoticeSimShort:"この部屋は試作エンジンで動いています。価格の比較は暗号化せず平文で行います。",
  engineNoticeSim:"この部屋は試作エンジンで動いています。注文と方針を分けて配る処理、不正の訂正、確保と決済の流れは実装どおりですが、価格の比較は暗号化せず平文で行います。席ごとに見える情報を絞っているのは画面上の投影で、秘密計算による保証ではありません。以下の「分割された値だけ」という説明は設計の説明です。",
  engineNoticeMpc:"この部屋は秘密計算（MP-SPDZ）で価格を比較しています。席ごとの表示の絞り込みは画面上の投影で、秘匿の保証は計算側の設計によります。",
  designNote:"設計の説明",
  engineNodeSim:"試作エンジン：このノードが持つ分割した値は実装どおりですが、価格の比較は部屋側が平文で行います。",
  legendSim:"試作エンジン：価格の比較は平文です。分割・訂正・確保・決済の流れは実装どおりです。",
  whatIsThis:"このデモについて",
  publicTitle:"公開情報", publicWhy:"ラウンドの終了後に、どの席からも確認できる情報です。",
  historyTitle:"最近のラウンド",
  seatsTitle:"参加者", seatsWhy:"各席の状態です。",
  noticesTitle:"通知",
  taken:"使用中", auto:"自動", manual:"操作中",
  seatTaker:"注文者", seatMaker:"値付け", seatNode:"計算ノード", seatObserver:"全体表示",
  // phases
  phaseDeal:"注文と気配の方針を分けて配る", phaseCheck:"入力が正しいか確かめる", phaseReduce:"中身を見ずに価格を比べる",
  phaseReduceSim:"価格を比べる（試作：平文で比較）",
  phaseOpen:"注文者だけが読める結果を返す", phaseSettle:"証明を確かめて DeFMI で決済", phaseIdle:"待機中", phaseDone:"完了", phaseQueued:"復旧待ち（未成立）",
  stopped:"停止", silent:"不参加", opened:"公開される暗号文", everyoneSees:"平文は非公開",
  idleManual:"注文者の送信を待っています。",
  idleAuto:"次のラウンドを待っています。",
  noRoundYet:"まだラウンドがありません。",
  ph_deal:(f)=>`注文と価格方針を${f.values}個の値に分け、${f.nodes}台の計算ノードへ1つずつ配布しています。`,
  ph_check:(f)=>`配布された${f.values}×${f.nodes}件の値が配布記録と一致するか検査しています。`,
  ph_check_skipped:"入力の検査は無効です。配られたのと違う値を使うノードを誰も止められません。",
  ph_check_rejected:(f)=>`${nodeList(f.rejected)}の入力が配布記録と一致しません。計算前に除外し、停止しました。`,
  ph_reduce:(f)=>`分割した値で${f.products}回の掛け算を実行し、開いた値を検査しています。価格の比較そのものは試作エンジンが平文で行っています。`,
  ph_reduce_mpc:(f)=>`MP-SPDZ が回路を実行しました（${f.rounds}ラウンド・${f.mb} MB）。`,
  ph_reduce_corrected:(f)=>` ${f.corrections}回を訂正し、${nodeList(f.named)}を特定しました。`,
  ph_open:"照合結果を注文者だけが読める形にして返しています。",
  ph_settle_settled:"zkPIの検証に合格しました。DeFMIが予約済みの在庫と資金を一つの取引で同時に更新しました。",
  ph_settle_queued:"計算ノードの復旧待ちです。注文と法人内の予約枠を保持し、復旧後にDeFMI正式予約を作って自動再送します。",
  ph_settle_released:(f)=>`予約を解放しました。${f.reason}`,
  ph_settle_cover:"ダミー通信のため、台帳は変更しません。",
  ph_done:(f)=>`ラウンド #${f.number} 完了。`,
  ph_queued:(f)=>`ラウンド #${f.number} はまだ完了していません。`,
  computeMs:(ms)=>`処理時間 ${formatDuration(ms, 'ja')}`,
  // graph
  graphAria:"取引の流れを示すネットワーク図",
  gTaker:"注文者", gMaker:"値付け", gNode:"計算ノード", gMatcher:"価格照合",
  gZkpi:"決済の証明（zkPI）", gLedger:"DeFMI 台帳", you:"あなた",
  productBoundary:"設計では、受付の順番、各社の確保枠、証明済みの zkPI、同時決済（DvP）の記録を Avalanche 上の DeFMI に記録します。計算ノードは資産を持ちません。",
  ledgerLocal:"この部屋の台帳はプロセス内の試験用台帳です。表示する台帳の識別値はその値で、Avalanche 上の DeFMI への記録ではありません。",
  ledgerNetwork:"今回の決済には、接続先 DeFMI ネットワークの確定記録が返っています。画面の在庫・資金の識別値と、ネットワークの記録識別子は別の値です。",
  ledgerUnknown:"台帳の記録先は接続先サーバーの構成に従います。この画面の接続ポートだけでは、記録先を判定できません。",
  ledgerGateway:"台帳の記録先はゲートウェイ側の構成に従います（DeFMI ネットワーク構成では Avalanche 上の DeFMI）。",
  candidate:"最良気配の候補（未約定）", candidatePrice:"比較で選ばれた価格（未約定）",
  ledgerDvp:"同時引き渡し", ledgerReleased:"確保分を解放", ledgerHeld:"保留", ledgerUnchanged:"変更なし", ledgerNone:"—",
  legendNoCustody:"計算ノードは在庫も資金も保有しません。届くのは分割された値だけです。",
  legendLines:"動く線: いま流れている情報 ・ 淡い線: この席には見えない経路",
  eShares:"分割した値", eMyOrder:"あなたの注文を分割", eMyPolicy:"あなたの方針を分割",
  eCompute:"分割したまま計算", eOpen:"鍵付きの結果", eVerify:"検証", eLedger:"台帳を更新",
  eSettleTaker:"決済: 在庫と資金", eRelease:"予約を解放", eQueued:"予約を保持して復旧待ち", eSettleMaker:"決済: 約定分",
  eNoChange:"変更なし",
  nIdle:"待機", nReceiving:"受取中", nChecking:"検査中", nComputing:"計算中",
  nDone:"完了", nSilent:"不参加", nRejected:"除外", nNamed:"不正を訂正", nStopped:"停止", nQueued:"復旧待ち",
  mCompare:"全社の気配を比較", mCompareSim:"全社の気配を比較（平文）", mPick:"最良気配を選定", mNone:"該当なし",
  zPass:"検証に合格", zFail:"検証に不合格", zDecoded:"開いた値の検査に合格",
  zStopped:"不合格・停止", zWait:"待機",
  lSettled:"決済完了", lReleased:"予約を解放", lQueued:"予約を保持", lCover:"ダミー・変更なし", lWait:"待機",
  orderHidden:"注文は非公開", policyHidden:"方針は非公開",
  activeShort:"稼働", inactiveShort:"停止", maxShort:"最大", winner:"約定相手",
  // portfolio
  pfCash:"資金", pfAvailable:"利用可能", pfReserved:"予約中", pfTotal:"合計",
  pfDeltaReserve:"予約", pfDeltaSettle:"決済", pfDeltaRelease:"解放", pfDeltaUpdate:"予約枠更新",
  pfNodeNote:"このノードは在庫も資金も保有しません。届くのは分割された値だけで、元の注文や価格は復元できません。",
  pfObserverTitle:"全参加者の在庫と資金（デモ専用の全体表示）",
  pfMine:"あなたの在庫と資金",
  // chat
  chatTitle:"チャットで指示",
  chatWhyTaker:"日本語の指示を、上の入力欄と同じ「注文の設定」と「送信」に変換します。送る前に内容を確認できます。",
  chatWhyMaker:"日本語の指示を、上の設定と同じ「気配の方針の更新」に変換します。最大数量や稼働を変えると予約枠も変わります。送る前に内容を確認できます。",
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
  takerTitle:"注文を入力する",
  takerWhy:"送信すると資金または在庫を予約し、注文を分割して各計算ノードへ送ります。",
  asset:"銘柄", side:"売買", buy:"買い", sell:"売り", qty:"数量",
  limitPrice:"価格条件", limitPriceBuy:"買う上限の価格", limitPriceSell:"売る下限の価格",
  limitBuy:"この価格以下なら自動決済", limitSell:"この価格以上なら自動決済",
  kind:"種別", real:"実注文", cover:"ダミー通信",
  coverWhy:"ダミー通信は実注文と同じ計算・通信量・時間で走ります。外からは区別できません。",
  automaticSettle:"送信時に法人内の枠を確保します。MPCが停止中なら暗号化キューで保持し、復旧後にDeFMI正式予約とMPC受付へ自動で進みます。追加の署名は要りません。",
  submit:"送信する", waiting:"実行中…", reserveOnSend:"送信時に予約",
  openedTitle:"暗号化された約定結果",
  openedWhy:"全員が見られるのは暗号文だけです。約定価格を読めるのは、鍵を持つ注文者だけです。",
  minusMask:"あなたの鍵で読む",
  yourPrice:"約定価格", winnerIs:"約定相手",
  noMaker:"価格条件に合う約定はありませんでした",
  eligible:"条件を満たした相手数",
  coverRound:"このラウンドはダミー通信でした。価格は使われません。",
  settlementTitle:"照合と決済", noSettlement:"まだ決済結果はありません。",
  status:"状態", settled:"決済完了", released:"予約を解放", queued:"MPC復旧待ち", coverStatus:"ダミー（残高変更なし）",
  settlementCash:"受渡資金", stateRoot:"台帳の識別値", queueStateRoot:"画面上の予約記録", automatic:"追加署名なしで決済",
  sr_cover:"ダミー通信のため台帳は変更していません。",
  sr_mpc_aborted:"計算が安全に停止したため予約を解放しました。",
  sr_mpc_queued:"注文と法人内の予約枠を暗号化キューに保持しています。復旧後にDeFMI正式予約を作り、自動再送します。",
  sr_no_maker:"条件を満たす相手がいないため予約を解放しました。",
  sr_price_limit:"約定価格が上限の外だったため予約を解放しました。",
  sr_automatic_dvp:"予約済みの在庫と資金を台帳が同時に更新しました。",
  price:"価格",
  // maker
  makerTitle:"気配の方針（値付けの設定）",
  makerWhy:"注文は見えません。気配の方針と最大数量を先に登録しておくと、見えない注文に対して自動で評価されます。",
  ask_level:"基準価格からのずらし幅", spread:"売値と買値の差（スプレッド）", slope:"数量が増えたときの上乗せ", invcoef:"在庫に応じた調整の強さ",
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
  nodeTitle:"このノードが持っている値",
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
  observerWhy:"実運用にはない画面です。デモのために全席の情報をまとめて表示しています。",
  observerPanes:"気配の一覧は「価格照合」の札、決済の記録は「DeFMI 台帳」の札、各席の中身はその席の札から見られます。",
  settingsWhy:"部屋の進み方を変えます。全体表示の席だけが操作できます。",
  behavioursWhy:"各計算ノードが選んでいる動作です。実運用ではそのノードにしか見えません。",
  allQuotes:"全参加者の気配", maker:"値付け", node:"ノード",
  reason:"理由", request:"注文", behaviours:"ノードの動作", settings:"設定",
  roundEvery:"自動ラウンド間隔（秒）", stepMs:"段階ごとの表示時間（ミリ秒）",
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
  n_claimed:(f)=>`${seatName(f.seat)}の席に着席${f.label?"："+f.label:""}`,
  vdPassed:"MPC出力は参照値と一致しました",
  n_you_won:(f)=>`#${f.number} あなたが約定しました`,
  n_settled:(f)=>`#${f.number} 予約済みの在庫と資金を追加署名なしで決済しました`,
  n_reserve_released:(f)=>`#${f.number} ${t('sr_' + (f.reason||'no_maker'))}`,
  n_reserve_queued:(f)=>`#${f.number} 法人キューで復旧待ち。予約は保持しています。`,
  n_limit_released:(f)=>`#${f.number} 約定価格が上限の外だったため予約を解放しました`,
  a_beyond_capacity:(f)=>`応答${f.answered}台に対し、訂正できるのは${f.capacity}台まで`,
  a_absent:(f)=>`${nodeList(f.who)} が不参加。入力は全${f.n}台の値の和なので、1つ欠けると値が失われます`,
  a_commitment:(f)=>`${nodeList(f.who)} の入力が配布記録と一致しません`,
  a_too_few:(f)=>`${f.answered}台では復元に足りません（${f.needed}台必要）`,
  a_mismatch:(f)=>`結果が参照値と一致しません${f.detail?"："+f.detail:""}`,
  a_engine:(f)=>`外部エンジンが失敗しました${f.detail?"："+f.detail:""}`,
  a_queued:()=>"計算ノードの復旧待ち。法人キューから自動再送します。",
  explain:[
    ["注文者（Taker）","取引したい注文を出す側です。設計では注文は分割されて送られ、どの計算ノードにも全体は渡りません。約定価格を受け取れるのはこの席だけです。"],
    ["値付け（Maker）","売り買いの気配の方針と最大数量を先に登録し、注文の相手になる側です。他の席の画面に注文の中身は表示されません。約定すると確保しておいた枠から自動で決済されます。"],
    ["計算ノード","設計では分割された値だけを使って計算します。在庫や資金は持ちません。不正な動作を試すこともできます。"]
  ]
},
en: {
  title:"QOMM", subtitle:"A demo of matching a trade with the best quote while the order itself stays hidden.",
  engineSim:"simulation (cleartext compare)", engineSimShort:"sim", engineSimNote:"real share layer; the price comparison is in the clear",
  engineMpc:"secure computation (MPC)", engineMpcShort:"MPC", engineMpcNote:"the quote circuit runs under secure computation (MP-SPDZ) and hands a verified zkPI to DeFMI",
  leave:"leave", langOther:"JA",
  connecting:"connecting to the server…",
  reconnecting:"connection lost — reconnecting…",
  viewLabel:"view", viewPublic:"not seated (public information only)", viewHelp:"pick or switch a seat",
  myControls:"controls", close:"close", more:"more", less:"less",
  selectHint:"Enter opens",
  hintSeatless:"Pick a seat card in the diagram (taker, maker, node) to join. Pick a process card to see its public information.",
  hintSeated:"The outlined card is your seat. Pick a card to operate it.",
  hintObserver:"The observer view is demo-only full information. Pick a card to see a seat's contents; the settings are under \"settings\" above.",
  hintKeys:"Tab moves between cards, Enter opens, Esc closes.",
  paneJoinKicker:"join", paneSwitchTitle:"Switch seat · who is here",
  paneSwitchWhy:"Picking another seat leaves the current one and moves you there.",
  paneMine:"your seat", paneProcess:"process",
  seatAuto:"This seat is running automatically right now.",
  seatHeldBy:(f)=>`Held by ${f.who}.`,
  seatPrivate:"What this seat receives (order, policy, split values) is sent only to the screen of the person in it.",
  claimHere:"Take this seat", claimSwitch:"Move to this seat",
  observerSeatWhy:"In production the contents of this seat are not visible from any other seat. Only the observer view collects them.",
  obsPolicy:"registered policy", obsQuote:"this seat's quote", obsBehaviour:"behaviour", obsBalances:"inventory and cash",
  eligibleShort:"eligible",
  paneMatcherWhy:"Compares every quote with the order and picks the best counterparty that meets the price condition.",
  paneMatcherWhySim:"Compares every quote with the order and picks the best counterparty that meets the price condition. In this simulation engine the comparison runs in the clear.",
  paneZkpiWhy:"Produces the proof that the match was computed correctly; the ledger checks it.",
  paneLedgerWhy:"Records only verified trades, as a simultaneous delivery of the pre-reserved inventory and cash.",
  paneObserverTitle:"Settings and every participant",
  techNotes:"Technical notes", publicRecord:"ledger record",
  zkpiTech:"By design, a zkPI is a proof that the secure computation's result was computed from the right inputs, without revealing them. The ledger checks it before moving the reserved assets. Where this room actually records is stated in the notes below.",
  legendHeading:"legend", legendMore:"notes and technical limits",
  lobbyTitle:"Take a seat",
  lobbyWhy:"Each seat sees different information. That difference is the point. Empty seats run automatically.",
  yourName:"your name (optional)", watch:"watch everything (demo only, all seats)",
  theSeats:"empty seats run automatically; named ones have a person",
  roleTaker:"Requests a trade (Taker)", roleTakerWhy:"Sends the order. Only this seat receives the matched price.",
  roleMaker:"Provides quotes (Maker)", roleMakerWhy:"Registers a quoting policy and a maximum size up front; never sees the order.",
  roleNode:"Compute nodes", roleNodeWhy:"Compute on split values only. Hold no inventory or cash.",
  roleNodeWhySim:"By design they compute on split values only. In this simulation engine the price comparison is done by the room (server) in the clear. They hold no inventory or cash.",
  engineNoticeSimShort:"This room runs the simulation engine. The price comparison runs in the clear.",
  engineNoticeSim:"This room runs the simulation engine. Splitting and dealing, error correction, reserves and settlement work as implemented, but the price comparison runs in the clear, not under secure computation. Per-seat views are a screen projection, not a guarantee from secure computation. Statements below about \"split values only\" describe the design.",
  engineNoticeMpc:"This room compares prices under secure computation (MP-SPDZ). Per-seat views are a screen projection; the confidentiality guarantee comes from the computation design.",
  designNote:"Design description",
  engineNodeSim:"Simulation engine: the split values this node holds are real, but the price comparison is done by the room in the clear.",
  legendSim:"Simulation engine: the price comparison is in the clear. Splitting, correction, reserves and settlement work as implemented.",
  whatIsThis:"About this demo",
  publicTitle:"Public information", publicWhy:"What every seat can see after a round.",
  historyTitle:"Recent rounds",
  seatsTitle:"Participants", seatsWhy:"State of each seat.",
  noticesTitle:"Notices",
  taken:"taken", auto:"auto", manual:"held",
  seatTaker:"taker", seatMaker:"maker", seatNode:"node", seatObserver:"observer",
  phaseDeal:"split & deal", phaseCheck:"check inputs", phaseReduce:"price & match",
  phaseReduceSim:"price & match (sim: cleartext compare)",
  phaseOpen:"keyed result", phaseSettle:"verify proof & settle on DeFMI", phaseIdle:"idle", phaseDone:"done", phaseQueued:"waiting for recovery (not settled)",
  stopped:"stopped", silent:"absent", opened:"public ciphertext", everyoneSees:"plaintext stays private",
  idleManual:"Waiting for the taker to send.",
  idleAuto:"Waiting for the next round.",
  noRoundYet:"No round yet.",
  ph_deal:(f)=>`${f.values} values split into ${f.nodes} pieces, one per node.`,
  ph_check:(f)=>`Checking ${f.values} × ${f.nodes} pieces against the dealing records.`,
  ph_check_skipped:"Input check is off: nothing stops a node using a value it was not dealt.",
  ph_check_rejected:(f)=>`${nodeList(f.rejected)} stated a value the dealing record does not bind. Excluded and stopped.`,
  ph_reduce:(f)=>`${f.products} multiplications on split values; the openings are decoded and checked. The price comparison itself runs in the clear (simulation engine).`,
  ph_reduce_mpc:(f)=>`MP-SPDZ ran the circuit (${f.rounds} rounds, ${f.mb} MB).`,
  ph_reduce_corrected:(f)=>` ${f.corrections} corrected, named ${nodeList(f.named)}.`,
  ph_open:"The result is revealed under a key only the taker can remove.",
  ph_settle_settled:"Checked. DeFMI moved both pre-reserved legs atomically in one transaction.",
  ph_settle_queued:"Waiting for the MPC committee. The corporate outbox retains the order and internal reserve, then creates the DeFMI reservation during automatic replay.",
  ph_settle_released:(f)=>`Reserve released. ${f.reason}`,
  ph_settle_cover:"Dummy traffic: the ledger is unchanged.",
  ph_done:(f)=>`Round #${f.number} done.`,
  ph_queued:(f)=>`Round #${f.number} is not complete yet.`,
  computeMs:(ms)=>`completed in ${formatDuration(ms, 'en')}`,
  graphAria:"network diagram of the trade flow",
  gTaker:"Taker", gMaker:"Maker", gNode:"Node", gMatcher:"Match",
  gZkpi:"Settlement proof (zkPI)", gLedger:"DeFMI ledger", you:"you",
  productBoundary:"By design, request order, corporate reserve limits, proved zkPI instructions and DvP settlement are recorded by the DeFMI custom VM on Avalanche. MPC nodes never custody assets.",
  ledgerLocal:"This room's ledger is an in-process demo ledger; the ledger hash shown is its value, not a record on DeFMI on Avalanche.",
  ledgerNetwork:"This settlement includes a finality record from the connected DeFMI network. The portfolio state identifier and the network record identifier are separate values.",
  ledgerUnknown:"The connected server determines where the ledger is recorded. The browser connection port alone does not identify that destination.",
  ledgerGateway:"Where the ledger is recorded follows the gateway's configuration (DeFMI on Avalanche in the DeFMI network setup).",
  candidate:"best-quote candidate (not filled)", candidatePrice:"price picked by the comparison (not filled)",
  ledgerDvp:"DvP", ledgerReleased:"reserve released", ledgerHeld:"held", ledgerUnchanged:"unchanged", ledgerNone:"—",
  legendNoCustody:"Nodes hold no inventory or cash; only split values reach them.",
  legendLines:"moving line: information in flight · faint line: a path this seat cannot see",
  eShares:"split values", eMyOrder:"your order, split", eMyPolicy:"your policy, split",
  eCompute:"computed on split values", eOpen:"keyed result", eVerify:"verify", eLedger:"ledger update",
  eSettleTaker:"settle: inventory & cash", eRelease:"reserve released", eQueued:"reserve held; awaiting recovery", eSettleMaker:"settle: fill",
  eNoChange:"no change",
  nIdle:"idle", nReceiving:"receiving", nChecking:"checking", nComputing:"computing",
  nDone:"done", nSilent:"absent", nRejected:"excluded", nNamed:"corrected", nStopped:"stopped", nQueued:"awaiting recovery",
  mCompare:"comparing quotes", mCompareSim:"comparing quotes (cleartext)", mPick:"best quote picked", mNone:"no match",
  zPass:"matches reference", zFail:"does not match", zDecoded:"openings decoded",
  zStopped:"failed — stopped", zWait:"idle",
  lSettled:"settled", lReleased:"released", lQueued:"reserve held", lCover:"dummy — unchanged", lWait:"idle",
  orderHidden:"order not visible", policyHidden:"policy not visible",
  activeShort:"on", inactiveShort:"off", maxShort:"max", winner:"winner",
  pfCash:"settlement cash", pfAvailable:"available", pfReserved:"reserved", pfTotal:"total",
  pfDeltaReserve:"reserve", pfDeltaSettle:"settle", pfDeltaRelease:"release", pfDeltaUpdate:"reserve update",
  pfNodeNote:"This node holds no inventory or cash. Only split values arrive; the order and price cannot be rebuilt from them.",
  pfObserverTitle:"Every participant's balances (demo-only view)",
  pfMine:"Your inventory and cash",
  chatTitle:"Chat control",
  chatWhyTaker:"Plain sentences become the same order settings and send command as the form above. You confirm before anything is sent.",
  chatWhyMaker:"Plain sentences become the same policy updates as the settings above. Max size and on/off also change your reserve. You confirm before anything is sent.",
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
  automaticSettle:"Sending secures the corporate limit. If MPC is down, the encrypted outbox retains it and automatically proceeds to the DeFMI reservation and MPC admission after recovery. No further signature is needed.",
  submit:"send", waiting:"running…", reserveOnSend:"reserved on send",
  openedTitle:"Encrypted match result",
  openedWhy:"Everyone can see only the ciphertext. Only the taker holding the key can read the matched price.",
  minusMask:"read with your key",
  yourPrice:"matched price", winnerIs:"counterparty",
  noMaker:"no trade met the price condition",
  eligible:"eligible counterparties",
  coverRound:"that round was dummy traffic; the price is not used.",
  settlementTitle:"Match & settlement", noSettlement:"No settlement result yet.",
  status:"status", settled:"settled", released:"released", queued:"waiting for MPC", coverStatus:"dummy (no balance change)",
  settlementCash:"cash delivered", stateRoot:"ledger hash", queueStateRoot:"screen reserve record", automatic:"settled without a further signature",
  sr_cover:"Dummy traffic — the ledger is unchanged.",
  sr_mpc_aborted:"The computation stopped safely; the reserve was released.",
  sr_mpc_queued:"The encrypted corporate outbox retains the order and internal reserve. After recovery it creates the DeFMI reservation and replays automatically.",
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
  observerWhy:"Not present in production. Every seat's information collected for the demo.",
  observerPanes:"All quotes are on the \"Match\" card, settlement records on the \"DeFMI ledger\" card, and each seat's contents on that seat's card.",
  settingsWhy:"Changes how the room runs. Only the observer seat can change them.",
  behavioursWhy:"The behaviour each node has chosen. In production only that node can see it.",
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
  n_claimed:(f)=>`${seatName(f.seat)} seat taken${f.label?" by "+f.label:""}`,
  vdPassed:"MPC output matches the reference",
  n_you_won:(f)=>`#${f.number} you were filled`,
  n_settled:(f)=>`#${f.number} settled without a further signature`,
  n_reserve_released:(f)=>`#${f.number} ${t('sr_' + (f.reason||'no_maker'))}`,
  n_reserve_queued:(f)=>`#${f.number} queued until MPC recovery; the reserve remains held`,
  n_limit_released:(f)=>`#${f.number} released — price outside the limit`,
  a_beyond_capacity:(f)=>`${f.answered} nodes answered; capacity is ${f.capacity}`,
  a_absent:(f)=>`${nodeList(f.who)} absent; all ${f.n} inputs are needed`,
  a_commitment:(f)=>`${nodeList(f.who)} failed the input check`,
  a_too_few:(f)=>`${f.answered} nodes are not enough (need ${f.needed})`,
  a_mismatch:(f)=>`result does not match the reference${f.detail?": "+f.detail:""}`,
  a_engine:(f)=>`the external engine failed${f.detail?": "+f.detail:""}`,
  a_queued:()=>"waiting for MPC recovery; automatic replay remains pending",
  explain:[
    ["Taker","sends the order. By design it is split so no single node sees it all. Only this seat gets the price."],
    ["Maker","registers a price policy and a maximum size; the order is not shown on other seats' screens. A win settles from the reserve automatically."],
    ["Node","by design computes on split values. Holds no inventory or cash. Can misbehave on purpose."]
  ]
}};
// The room's engine decides what the screen may claim: 'sim' keeps the share
// layer but compares prices in the clear, so design statements get qualified.
function isSim(){ return !!(V && V.config && V.config.engine === 'sim'); }
// A frontend started with a private gateway carries its port in the page; a
// standalone room has none and settles on its own in-process demo ledger.
function gatewayConfigured(){
  return typeof document !== 'undefined'
    && !!(document.querySelector('meta[name="qomm-gateway-port"]')?.content || '').trim();
}
function ledgerNote(){
  const record = V && V.public && V.public.engine_stats && V.public.engine_stats.defmi_product_settlement;
  if (record && record.block_id && record.transaction_id) return t('ledgerNetwork');
  return t(gatewayConfigured() ? 'ledgerGateway' : 'ledgerUnknown');
}
function ledgerOutcome(status){
  return status === 'settled' ? t('ledgerDvp') : status === 'released' ? t('ledgerReleased')
    : status === 'queued' ? t('ledgerHeld') : status === 'cover' ? t('ledgerUnchanged') : t('ledgerNone');
}
function engineKey(key){ return isSim() && S[lang] && S[lang][key + 'Sim'] !== undefined ? key + 'Sim' : key; }

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
function formatDuration(value, language){
  const ms = Number(value);
  if (!Number.isFinite(ms) || ms < 0) return '--';
  const seconds = ms / 1000;
  if (language === 'en'){
    if (seconds < 60) return `${seconds < 1 ? seconds.toFixed(2) : seconds.toFixed(1)} s`;
    const minutes = Math.floor(seconds / 60);
    return `${minutes} min ${(seconds - minutes * 60).toFixed(1)} s`;
  }
  if (seconds < 60) return `${seconds < 1 ? seconds.toFixed(2) : seconds.toFixed(1)}秒`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}分${(seconds - minutes * 60).toFixed(1)}秒`;
}
// Seat identifiers on the wire are `taker`, `maker:N`, `node:N`, `observer`.
// Notices must show the seat the way the rest of the page names it.
function seatName(seat){
  const [kind, index] = String(seat || '').split(':');
  const label = kind === 'taker' ? t('seatTaker') : kind === 'maker' ? t('seatMaker')
    : kind === 'node' ? t('seatNode') : kind === 'observer' ? t('seatObserver') : kind;
  return index === undefined ? label : `${label} ${index}`;
}
// The gateway reports verification outcomes as fixed English sentences; show
// the known ones in the page language and pass unknown text through.
function verifiedDetailText(detail){
  if (detail === 'MPC output verification passed') return t('vdPassed');
  return detail;
}
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
function isQueuePending(pub){
  return !!pub && (pub.abort_code === 'queued'
    || (pub.settlement && pub.settlement.status === 'queued'));
}
function translateRefusal(reason){
  const r = String(reason || '');
  let m;
  if (/Avalanche transaction \S+ was not accepted in time/.test(r))
    return lang === 'ja' ? '台帳の確定待ちがタイムアウトしました。注文の成立はまだ確認できていません。' : 'Ledger finality timed out. Order completion is not yet confirmed.';
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

let ws = null, V = null, P = null, built = '', lastDraw = '', lastError = '', lastErrorTone = 'bad';
let session = '';
if (hasDom){ try { session = localStorage.getItem('qomm.session') || ''; } catch (e) { session = ''; } }
let chatHistory = [], chatPending = null, chatAwaitingReply = false;
// Keep edits visible until the server echoes them; an older periodic view can
// arrive after blur and must not replace the value the user is about to submit.
let takerEdits = {};
let pfPrev = null, pfPrevRound = null, pfRoundBaseline = null;
let pfDelta = {}, pfDeltaPhase = '', pfDeltaKind = '';
let reducedMotion = false, lastGraphWidth = -1;
// Graph-first frame state. The drawer shows one pane: 'seats' (pick or switch
// a seat, who is here, notices), 'me' (the observer's controls), or the id of
// the card selected in the diagram ('taker', 'maker:3', 'node:0', 'matcher',
// 'zkpi', 'ledger'). null keeps it closed; undefined means the first view has
// not decided yet.
let drawerPane;
let lastSeatId = null, seatsSig = '', lastModel = null, lastInset = 0, techOpen = false;
let guided = false, hintClosed = false;
if (hasDom){ try { guided = localStorage.getItem('qomm.guided') === '1'; } catch (e) { guided = false; } }
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
  if (ws && ws.readyState === 1){
    if (message.type === 'request' && V && V.kind === 'taker') {
      if (message.values && Object.hasOwn(message.values, 'asset')) takerEdits = {};
      Object.assign(takerEdits, message.values || {});
    }
    if (message.type === 'submit' || message.type === 'submit_any') {
      P = {active:true, local:true, status:'running', stage:'sending', elapsed_ms:0, stage_ms:0, nodes:[]};
      setError('', 'bad');
      drawExecutionProgress();
    }
    ws.send(JSON.stringify(message)); return true;
  }
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
  const configuredPort = (document.querySelector('meta[name="qomm-gateway-port"]')?.content || '').trim();
  const gatewayHost = configuredPort ? location.hostname + ':' + configuredPort : location.host;
  ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://')
                     + gatewayHost + '/ws?' + q.toString());
  ws.onmessage = (e) => {
    let m; try { m = JSON.parse(e.data); } catch (err) { return; }
    if (m.type === 'view'){
      V = m; session = m.session;
      if (m.kind !== 'taker') takerEdits = {};
      else for (const key of Object.keys(takerEdits)) {
        if (m.taker && m.taker.pending && m.taker.pending[key] === takerEdits[key]) delete takerEdits[key];
      }
      if (m.progress && (!P || !P.local || m.progress.active)) P = m.progress;
      try { localStorage.setItem('qomm.session', session); } catch (err) { /* private mode */ }
      if (lastErrorTone === 'warn') setError('', 'bad');
      render();
    } else if (m.type === 'progress'){
      P = m;
      drawExecutionProgress();
    } else if (m.type === 'challenge_receipt'){
      $('challenge-claim').disabled = true;
      setError(lang === 'ja' ? 'challengeを台帳で受理しました。元の証明による応答を待っています。' : 'Challenge accepted on the ledger. Waiting for proof.', 'warn');
    } else if (m.type === 'action_error'){
      setError(m.reason || 'Action refused', 'bad');
      drawAssurance();
    } else if (m.type === 'refused'){
      takerEdits = {};
      if (P && P.active) P = {...P, active:false, status:'failed'};
      const text = translateRefusal(m.reason || 'request refused');
      setError(text, 'bad');
      if (chatAwaitingReply){ addChatMessage(tf('chatRefused', text), 'error'); chatAwaitingReply = false; }
      // A rejected action may leave the authoritative view byte-for-byte
      // unchanged. Force a redraw so optimistic local control state (for
      // example the disabled submit button) returns to that view.
      lastDraw = '';
      render();
    }
  };
  ws.onclose = () => {
    if (P && P.active) P = {...P, active:false, status:'disconnected'};
    drawExecutionProgress();
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
function showGraphCash(value){
  if (value === null || value === undefined || value === '') return '--';
  const amount = Number(value);
  const absolute = Math.abs(amount);
  const compact = (unit, suffix) => {
    const scaled = amount / unit;
    const digits = Math.abs(scaled) >= 100 || Number.isInteger(scaled) ? 0 : Math.abs(scaled) >= 10 ? 1 : 2;
    return scaled.toLocaleString(undefined, {minimumFractionDigits: 0, maximumFractionDigits: digits}) + suffix;
  };
  if (lang === 'ja' && absolute >= 100000000) return compact(100000000, '億');
  if (lang === 'ja' && absolute >= 10000) return compact(10000, '万');
  return showCash(amount);
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
    : status === 'queued' ? t('queued')
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
  if (settlement.detail && settlement.status !== 'queued')
    box.appendChild(el('p', 'why mono', settlement.detail));
  if (settlement.state_root){
    box.appendChild(el('div', 'tag', t(settlement.status === 'queued' ? 'queueStateRoot' : 'stateRoot')));
    box.appendChild(el('div', 'root', settlement.state_root));
  }
}
function setVal(node, value){
  if (node && document.activeElement !== node) node.value = value;
}
function phaseIndex(phase){ return PHASE_ORDER.indexOf(phase); }
function isBusy(){ return !!(P && P.active) || !!(V && (V.busy || (V.phase !== 'idle' && V.phase !== 'done'))); }

const EXECUTION_LABELS = {
  sending: ['送信済み・サーバーの受付待ち', 'Sent · awaiting server'],
  policy: ['Maker方針の準備・台帳への登録', 'Preparing Maker policies and ledger registration'],
  reserve: ['注文の資金・在庫を予約', 'Reserving order funds or inventory'],
  admission: ['受付・入力の準備', 'Admission and input preparation'],
  mpc: ['7ノードで秘密計算', 'MPC on seven nodes'],
  proof: ['決済の証明を生成・検証', 'Generating and checking settlement proofs'],
  settlement: ['DeFMIへ送信・台帳の確定待ち', 'Submitted to DeFMI · awaiting finality'],
  reconcile: ['結果・残高の照合と保存', 'Reconciling and saving results and balances'],
  display: ['計算結果を画面に反映', 'Updating the displayed result'],
  waiting: ['待機', 'Waiting'], signature: ['署名処理', 'Signing'],
  input: ['入力の準備', 'Preparing inputs'], save: ['状態を保存', 'Saving state'],
  prepare: ['準備・照合', 'Preparing / reconciling'], public_keys:['公開鍵を確認','Checking public keys'],
};
function executionLabel(stage){
  const proofLabels = {
    quote_proof:['価格比較の証明','Quote proof'], zkpi_proof:['決済認可の証明','Settlement authorization proof'],
    limit_proof:['指値条件の証明','Price-limit proof'], pool_proof:['予約残高の証明','Reserve balance proof'],
    dvp_proof:['資金・在庫の同時移転の証明','Delivery-versus-payment proof'],
  };
  if (proofLabels[stage]) return proofLabels[stage][lang === 'ja' ? 0 : 1];
  return (EXECUTION_LABELS[stage] || EXECUTION_LABELS.waiting)[lang === 'ja' ? 0 : 1];
}
function executionState(status){
  const labels = {
    running:['実行中','Running'], done:['応答済み','Responded'], failed:['通信失敗','Call failed'], rejected:['否認応答','Request rejected'],
    interrupted:['中断','Interrupted'], disconnected:['接続切断・結果未確認','Disconnected · result unknown'], waiting:['待機','Waiting'],
  };
  return (labels[status] || labels.waiting)[lang === 'ja' ? 0 : 1];
}
function hasExecutionProgress(){ return !!(P && (P.started_at_ms || P.local)); }
let selectedExecution = null, executionHistory = null, executionHistoryError = '';
async function selectExecution(run){
  selectedExecution = run; executionHistory = null; executionHistoryError = '';
  fillDrawer();
  if (run === null) return;
  try {
    const port = (document.querySelector('meta[name="qomm-gateway-port"]')?.content || '').trim();
    const base = port ? location.protocol + '//' + location.hostname + ':' + port : location.origin;
    const response = await fetch(base + '/execution-history?run=' + encodeURIComponent(run), {cache:'no-store'});
    if (!response.ok) throw new Error('history unavailable');
    const detail = await response.json();
    if (selectedExecution !== run) return;
    executionHistory = detail;
  } catch (error) {
    if (selectedExecution !== run) return;
    executionHistoryError = lang === 'ja' ? '履歴を読み込めませんでした。もう一度選んでください。' : 'Could not load this run. Select it again to retry.';
  }
  if (drawerPane === 'gateway') fillDrawer();
}
function transferText(event){
  const node = (lang === 'ja' ? '計算ノード ' : 'Node ') + (event.nodes ? event.nodes.join(', ') : event.node);
  const responded = event.status === 'received' || event.status === 'rejected';
  const route = responded ? node + ' → Gateway' : 'Gateway → ' + node;
  const status = event.status === 'sent' ? (lang === 'ja' ? '送信開始' : 'Request started')
    : event.status === 'received' ? (lang === 'ja' ? '応答受信' : 'Response received')
    : event.status === 'rejected' ? (lang === 'ja' ? '否認応答を受信' : 'Rejection received')
    : (lang === 'ja' ? '通信失敗・応答未確認' : 'Call failed');
  const content = {
    public_keys:['公開鍵の照会','公開鍵'], prepare:['初期設定の要求','設定結果'],
    admission:['注文受付の要求','受付結果'], input:['秘密分散した入力','入力の読込結果'],
    mpc:['秘密計算の開始指示','秘密計算の結果'], proof:['証明生成の要求','証明の応答'],
    signature:['署名の要求','署名の応答'], save:['状態保存の要求','保存結果'],
  };
  const what = lang === 'ja' && content[event.step] ? content[event.step][event.status === 'received' ? 1 : 0]
    : executionLabel(event.step) + (event.status === 'received' ? (lang === 'ja' ? 'の応答' : ' response') : (lang === 'ja' ? 'の要求' : ' request'));
  return `${route} · ${what} · ${status}`;
}
function drawTransferSummary(){
  let button = $('transfer-summary');
  if (!button) {
    button = el('button', 'transfer-summary'); button.id = 'transfer-summary';
    button.type = 'button'; button.onclick = () => openPane('gateway');
    $('phase-caption').after(button);
  }
  const events = P && P.events || [], last = events[events.length - 1];
  const runs = P && P.runs || [];
  button.classList.toggle('hide', !last && !runs.length);
  if (last) button.textContent = (lang === 'ja' ? '直前の通信: ' : 'Latest: ') + transferText(last)
    + (lang === 'ja' ? '  › 履歴' : '  › History');
  else if (runs.length) button.textContent = (lang === 'ja' ? '実行履歴を開く · ' : 'Execution history · ') + runs.length + (lang === 'ja' ? '件' : ' runs');
  button.dataset.compactLabel = lang === 'ja' ? '通信・実行履歴' : 'Execution history';
}
function drawTransferHistory(box){
    const live = selectedExecution === null;
  const data = live ? P : executionHistory;
  if (!live) setDrawerHeading(lang === 'ja' ? '保存された実行' : 'Saved execution',
    (lang === 'ja' ? '実行 #' : 'Run #') + selectedExecution,
    data ? new Date(data.started_at_ms).toLocaleString(lang === 'ja' ? 'ja-JP' : 'en-US') : '');
  box.appendChild(el('h3', '', lang === 'ja' ? '実行履歴' : 'Execution history'));
  const follow = el('button', 'history-follow', lang === 'ja' ? '現在の実行をリアルタイムで見る' : 'Follow the current run');
  follow.type = 'button'; follow.disabled = live; follow.onclick = () => selectExecution(null);
  box.appendChild(follow);
  const runs = el('ol', 'execution-runs');
  for (const run of P && P.runs || []) {
    const row = el('li'), button = el('button', selectedExecution === run.run ? 'selected' : '');
    const state = run.status === 'done' ? (lang === 'ja' ? '完了' : 'Complete') : executionState(run.status);
    button.type = 'button'; button.setAttribute('aria-pressed', String(selectedExecution === run.run));
    button.textContent = (lang === 'ja' ? '実行 #' : 'Run #') + run.run + ' · ' + new Date(run.started_at_ms).toLocaleString(lang === 'ja' ? 'ja-JP' : 'en-US') + ' · ' + state + ' · ' + formatDuration(run.elapsed_ms, lang);
    button.onclick = () => selectExecution(P && P.active && P.run === run.run ? null : run.run);
    row.appendChild(button); runs.appendChild(row);
  }
  box.appendChild(runs);
  if (P && P.history_error) box.appendChild(el('p', 'transfer-failed', lang === 'ja' ? '履歴の保存に失敗しました。この実行は再起動後に残らない可能性があります。' : 'History could not be saved. This run may not survive a restart.'));
  if (!live && !data) {
    box.appendChild(el('p', '', executionHistoryError || (lang === 'ja' ? '履歴を読み込んでいます…' : 'Loading history…')));
    return;
  }
  if (!live) box.appendChild(el('h3', '', (lang === 'ja' ? '選択した実行 #' : 'Selected run #') + selectedExecution));
  box.appendChild(el('p', 'why', lang === 'ja'
    ? '青い流れは現在の工程、黄色は通信の応答待ち、緑は応答受信を表します。この履歴には実際のGateway通信を記録しています。台帳内や計算ノード間の通信は計測していません。'
    : 'Blue flows show the current operation, amber shows calls awaiting replies, and green shows received replies. This history records actual Gateway calls; ledger-internal and node-to-node traffic is not measured.'));
  const running = (data && data.nodes || []).filter(node => data.active && node.status === 'running');
  box.appendChild(el('h3', '', live ? (lang === 'ja' ? '現在' : 'Now') : (lang === 'ja' ? '終了時の状態' : 'Final state')));
  box.appendChild(el('p', '', running.length ? running.map(node => `${lang === 'ja' ? '計算ノード' : 'Node'} ${node.node}: ${executionLabel(node.step)}`).join(' / ')
    : data && data.active ? executionLabel(data.stage) : lang === 'ja' ? '処理中の通信はありません' : 'No active calls'));
  box.appendChild(el('h3', '', lang === 'ja' ? '直前の通信（新しい順）' : 'Recent calls (newest first)'));
  const list = el('ol', 'transfer-history');
  box.appendChild(el('p', 'why', lang === 'ja' ? '同じ工程・種類・方向の通信をまとめています。時刻は処理開始からの経過です。' : 'Grouped by stage, operation and direction. Times are elapsed since this run began.'));
  for (const event of [...(data && data.transfers || [])].sort((a,b) => b.last_ms - a.last_ms)) {
    const row = el('li', ['failed','rejected'].includes(event.status) ? 'transfer-failed' : '');
    row.appendChild(el('time', '', formatDuration(event.first_ms, lang) + (event.last_ms !== event.first_ms ? '–' + formatDuration(event.last_ms, lang) : '')));
    row.appendChild(el('span', '', transferText(event) + ` · ${event.count}${lang === 'ja' ? '件' : ' calls'}`));
    list.appendChild(row);
  }
  box.appendChild(list);
}
function drawExecutionProgress(){
  drawAssurance();
  if (!V) return;
  drawPhaseStrip();
  drawNetworkGraph();
  drawTransferSummary();
  if (drawerPane === 'gateway') fillDrawer();
  if (typeof takerEls !== 'undefined' && takerEls && takerEls.go) {
    takerEls.go.disabled = isBusy();
    takerEls.go.textContent = isBusy() ? t('waiting') : t('submit');
  }
}
/* Where an aborted round stopped, from the public abort code. */
function stoppedAt(pub){
  if (!pub || !pub.aborted) return null;
  const code = pub.abort_code || '';
  if (code === 'queued') return null;
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
      if (f.status === 'queued') return t('ph_settle_queued');
      if (f.status === 'cover') return t('ph_settle_cover');
      return tf('ph_settle_released', {reason: t('sr_' + (f.reason || 'no_maker'))});
    case 'done': {
      if (isQueuePending(pub))
        return tf('ph_queued', {number: f.number || pub.number || '--'}) + ' ' + abortText(pub);
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
  if (hasExecutionProgress()) {
    const stages = ['policy','reserve','admission','mpc','proof','settlement','reconcile'];
    const at = stages.indexOf(P.stage);
    stages.forEach((stage, index) => {
      const current = stage === P.stage;
      const cls = 'phase-step' + (current ? (P.status === 'failed' ? ' stopped' : ' on') : index < at ? ' done' : '');
      const step = el('span', cls, executionLabel(stage));
      step.setAttribute('role','listitem');
      if (current) step.setAttribute('aria-current','step');
      strip.appendChild(step);
    });
    const caption = $('phase-caption');
    const state = P.status === 'done' ? (lang === 'ja' ? '処理完了' : 'Complete') : executionState(P.status);
    caption.textContent = `${state} · ${executionLabel(P.stage)} · ${formatDuration(P.elapsed_ms || 0, lang)}`;
    if (P.active && !P.local) caption.appendChild(el('span','execution-stage-time',
      lang === 'ja' ? `（この工程 ${formatDuration(P.stage_ms || 0, lang)}）` : ` (stage ${formatDuration(P.stage_ms || 0, lang)})`));
    caption.classList.toggle('execution-failed', P.status === 'failed');
    return;
  }
  $('phase-caption').classList.remove('execution-failed');
  const pub = V.public || {};
  const queuePending = isQueuePending(pub);
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
    const step = el('span', cls, t(engineKey(key))); step.setAttribute('role', 'listitem');
    if (V.phase === id) step.setAttribute('aria-current', 'step');
    strip.appendChild(step);
  });
  const terminalClass = 'phase-step' + (V.phase === 'done' ? (queuePending ? ' queued' : ' on') : '');
  const terminalLabel = V.phase === 'idle' ? 'phaseIdle' : queuePending ? 'phaseQueued' : 'phaseDone';
  const done = el('span', terminalClass, t(terminalLabel));
  done.setAttribute('role', 'listitem');
  strip.appendChild(done);
  const caption = $('phase-caption');
  const noRound = !pub.number && V.phase === 'idle';
  caption.textContent = (noRound ? t('noRoundYet') + ' ' : '') + phaseCaption();
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

  // ── participants: one horizontal lane on desktop, a readable two-column
  //    stack on narrow screens.  Shrinking the 980px desktop topology made every
  //    label illegible on a phone even when no node was technically clipped.
  const rowA = 88;
  const tw = compact ? Math.min(224, W - 2 * padX) : 170;
  const th = 194;
  const takerX = compact ? W / 2 : padX + tw / 2;
  let mh = 194;
  const makerPositions = [];
  let mw, participantBottom, slim = false;
  if (compact){
    const makerCols = Math.max(1, Math.min(2, nMakers));
    mw = Math.min(154, (W - 2 * padX - gap * (makerCols - 1)) / makerCols);
    const makerRows = Math.max(1, Math.ceil(nMakers / makerCols));
    const makerY0 = rowA + th / 2 + 28 + mh / 2;
    for (let i = 0; i < nMakers; i++){
      const r = Math.floor(i / makerCols), c = i % makerCols;
      const count = Math.min(makerCols, nMakers - r * makerCols);
      const span = mw * count + gap * (count - 1);
      const start = padX + (W - 2 * padX - span) / 2;
      makerPositions.push({x: start + c * (mw + gap) + mw / 2, y: makerY0 + r * (mh + 10)});
    }
    participantBottom = makerY0 + (makerRows - 1) * (mh + 10) + mh / 2;
  } else {
    const makerAreaL = padX + tw + gap * 2, makerAreaR = W - padX;
    mw = Math.max(112, Math.min(154, (makerAreaR - makerAreaL - gap * (nMakers - 1)) / Math.max(nMakers, 1)));
    // Eight Makers in one lane leave ~112px per card: a two-column metric
    // grid ellipsizes every label there. Slim cards list the four figures as
    // label/value rows instead and get the extra height that needs.
    slim = mw < 140;
    if (slim) mh = 196;
    const makerSpan = mw * nMakers + gap * (nMakers - 1);
    const makerStart = makerAreaL + Math.max(0, (makerAreaR - makerAreaL - makerSpan) / 2);
    for (let i = 0; i < nMakers; i++) makerPositions.push({x: makerStart + i * (mw + gap) + mw / 2, y: rowA});
    participantBottom = rowA + Math.max(th, mh) / 2;
  }
  // ── row B: nodes across the full width, wrapping when there are many ──
  const perRowCapacity = Math.max(1, Math.floor((W - 2 * padX + gap) / ((compact ? 82 : 112) + gap)));
  // Seven nodes on one line technically fit, but their labels and state become
  // unreadable.  A 4+3 layout keeps the quorum visible as a network, not a row
  // of tiny boxes.  Large canvases may still use one row.
  const perRow = compact && nNodes > 2 ? 2 : W < 1200 && nNodes > 4 ? 4 : perRowCapacity;
  const nodeRows = Math.max(1, Math.ceil(nNodes / perRow));
  const nPerRow = Math.min(perRow, nNodes);
  const nw = Math.max(80, Math.min(compact ? 138 : 170, (W - 2 * padX - gap * (nPerRow - 1)) / nPerRow));
  const nh = 166;
  const rowB = participantBottom + 154 + nh / 2;
  const rowBBottom = rowB + (nodeRows - 1) * (nh + 10) + nh / 2;
  // ── row C: matcher → verify → ledger ──
  const cw = compact ? 174 : 200, ch = 170;
  const lw = compact ? 210 : 246;
  let rowC, rowD, rowE;
  if (wide){ rowC = rowD = rowE = rowBBottom + 50 + ch / 2; }
  else { rowC = rowBBottom + 58 + ch / 2; rowD = rowC + ch + 30; rowE = rowD + ch + 30; }
  const H = rowE + ch / 2 + 22;

  const add = (n) => { nodes.push(n); return n; };
  add({id: 'taker', type: 'taker', x: takerX, y: rowA, w: tw, h: th});
  for (let i = 0; i < nMakers; i++)
    add({id: 'maker:' + i, type: 'maker', index: i, x: makerPositions[i].x, y: makerPositions[i].y, w: mw, h: mh, slim});
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
  const gateway = add({id:'gateway', type:'gateway', x:W/2, y:participantBottom+70, w:compact ? 240 : 270, h:96});
  const byId = {}; nodes.forEach(n => { byId[n.id] = n; });
  const top = (n) => ({x: n.x, y: n.y - n.h / 2}), bottom = (n) => ({x: n.x, y: n.y + n.h / 2});
  const left = (n) => ({x: n.x - n.w / 2, y: n.y}), right = (n) => ({x: n.x + n.w / 2, y: n.y});
  const laneL = 12, laneL2 = 18, laneR = W - 12;
  const trunkY = participantBottom + 30, returnY = participantBottom + 14;
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
      edges.push({id: edgeId(p, to.id), source: p, target: to.id, stage: 'deal', state: dealState, own, color: 'teal',
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
    edges.push({id: edgeId(from.id, 'matcher'), source: from.id, target: 'matcher', stage: 'reduce', state: dead ? 'cut' : reduceState, own, color: 'teal',
      d: orthoPath([bottom(from), {x: from.x, y: trunk2Y}, {x: matcher.x, y: trunk2Y}, top(matcher)]),
      chain: [0, 1], particle: own && kind !== 'observer' || (kind === 'observer' && i === 0)});
  }
  if (reduceState === 'flow') labels.push({x: matcher.x, y: trunk2Y - 6, text: t('eCompute'), strong: true});
  // ── the keyed result back to the taker (everyone sees it; only the taker reads it) ──
  const openState = stageState('open');
  const takerN = byId.taker;
  edges.push({id: edgeId('matcher', 'taker'), source: 'matcher', target: 'taker', stage: 'open', state: openState, own: true, color: 'amber',
    d: orthoPath([left(matcher), {x: laneL, y: matcher.y}, {x: laneL, y: takerN.y}, left(takerN)]),
    chain: [0, 1], particle: true});
  if (openState === 'flow') labels.push({x: laneL + 6, y: (matcher.y + takerN.y) / 2, text: t('eOpen'), anchor: 'start', strong: true});
  // ── verify → ledger → participants ──
  const settleState = stageState('settle');
  const queuePending = pub.abort_code === 'queued' || settleStatus === 'queued';
  const aborted = !!pub.aborted && !queuePending;
  const chainLen = 3;
  edges.push({id: edgeId('matcher', 'zkpi'), source: 'matcher', target: 'zkpi', stage: 'settle', state: aborted ? 'cut' : settleState, own: true, color: 'blue',
    d: wide ? orthoPath([right(matcher), left(zkpi)]) : orthoPath([bottom(matcher), top(zkpi)]),
    chain: [0, chainLen], particle: true});
  edges.push({id: edgeId('zkpi', 'ledger'), source: 'zkpi', target: 'ledger', stage: 'settle', state: aborted ? 'cut' : settleState, own: true, color: 'blue',
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
    : settleStatus === 'released' ? t('eRelease') : settleStatus === 'queued' ? t('eQueued')
    : settleStatus === 'cover' ? t('eNoChange') : '';
  const ledgerToTaker = wide
    ? orthoPath([bottom(ledger), {x: ledger.x, y: bottomLane}, {x: laneL2, y: bottomLane}, {x: laneL2, y: takerN.y + 8}, {x: takerN.x - tw / 2, y: takerN.y + 8}])
    : orthoPath([left(ledger), {x: laneL2, y: ledger.y}, {x: laneL2, y: takerN.y + 8}, {x: takerN.x - tw / 2, y: takerN.y + 8}]);
  edges.push({id: edgeId('ledger', 'taker'), source: 'ledger', target: 'taker', stage: 'settle', state: settleStatus === 'cover' && settleState !== 'idle' ? 'done' : settleState, own: true,
    color: settleStatus === 'released' || settleStatus === 'queued' ? 'amber' : 'blue', d: ledgerToTaker, chain: [2, chainLen], particle: settleStatus !== 'cover' && !queuePending});
  if (settleState === 'flow' && takerSettleText)
    labels.push({x: laneL2 + 6, y: wide ? (bottomLane + takerN.y) / 2 : (ledger.y + takerN.y) / 2 + 14, text: takerSettleText, anchor: 'start', strong: true});
  // ledger → the winning maker, only where this seat may know who won
  if (winnerKnown !== null && byId['maker:' + winnerKnown] && settleStatus === 'settled'){
    const m = byId['maker:' + winnerKnown];
    edges.push({id: edgeId('ledger', m.id), source: 'ledger', target: m.id, stage: 'settle', state: settleState, own: true, color: 'blue',
      d: orthoPath([right(ledger), {x: laneR, y: ledger.y}, {x: laneR, y: returnY}, {x: m.x, y: returnY}, bottom(m)]),
      chain: [2, chainLen], particle: true});
    if (settleState === 'flow') labels.push({x: laneR - 6, y: (ledger.y + returnY) / 2, text: t('eSettleMaker'), anchor: 'end', strong: true});
  }

  // ── what each node says ──
  const orderText = describeOrder;
  nodes.forEach(n => {
    n.classes = []; n.title = ''; n.sub = ''; n.badge = '';
    const isMe = n.id === me;
    if (isMe) n.classes.push('is-me');
    if (n.type === 'gateway'){
      n.title = lang === 'ja' ? '進行役（Gateway）' : 'Coordinator (Gateway)';
      n.sub = hasExecutionProgress() ? executionLabel(P.stage) : executionLabel('waiting');
      if (P && P.active) n.classes.push('is-active');
    } else if (n.type === 'taker'){
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
      n.title = t('gMaker') + ' ' + n.index;
      const pol = kind === 'maker' && isMe ? maker && maker.policy : kind === 'observer' ? (obs && obs.policies || [])[n.index] : null;
      if (pol){
        n.sub = `${(V.assets[pol.asset] || {}).name || '--'} · ${pol.active ? t('activeShort') : t('inactiveShort')} · ${t('maxShort')} ${pol.maxqty}`;
        if (!pol.active) n.classes.push('is-off');
      } else n.sub = t('policyHidden');
      if (isMe) n.sub = t('you') + ' · ' + n.sub;
      if (dealState === 'flow' && (mine(n.id) || kind !== 'maker')) n.classes.push('is-active');
      if (winnerKnown === n.index && (phase === 'settle' || phase === 'done') && settleStatus === 'settled') n.classes.push('is-winner');
      if (compact) n.classes.push('compact');
      if (n.slim) n.classes.push('is-slim');
    } else if (n.type === 'node'){
      n.title = t('gNode') + ' ' + n.index;
      const live = !silent.has(n.index) && !rejected.has(n.index);
      let sub = t('nIdle');
      if (queuePending) { sub = t('nQueued'); n.classes.push('is-silent'); }
      else if (silent.has(n.index)) { sub = t('nSilent'); n.classes.push('is-silent'); }
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
      if (queuePending) { n.sub = t('nQueued'); n.classes.push('is-stopped'); }
      else if (phase === 'reduce') { n.sub = t(engineKey('mCompare')); n.classes.push('is-active'); }
      else if (stop && stopIndex <= phaseIndex('reduce') && at >= stopIndex) { n.sub = t('nStopped'); n.classes.push('is-stopped'); }
      else if (at >= phaseIndex('open')){
        // The comparison picks a candidate; only a settled round makes it the
        // counterparty. A released/queued/cover round shows the candidate.
        const pickLabel = settleStatus === 'settled' ? t('winner') : t('candidate');
        n.sub = kind === 'taker' && taker && taker.last ? (taker.last.winner === null || taker.last.winner === undefined ? t('mNone') : `${pickLabel}: ${t('gMaker')} ${taker.last.winner}`)
          : kind === 'observer' && obs ? (obs.winner === null || obs.winner === undefined ? t('mNone') : `${pickLabel}: ${t('gMaker')} ${obs.winner}`)
          : t('mPick');
        if (phase === 'open') n.classes.push('is-active');
      } else n.sub = t('nIdle');
    } else if (n.type === 'zkpi'){
      n.title = t('gZkpi');
      if (queuePending) n.sub = t('zWait');
      else if (phase === 'settle' || phase === 'done'){
        n.sub = pub.aborted ? t('zStopped') : pub.verified === true ? t('zPass') : pub.verified === false ? t('zFail') : t('zDecoded');
        if (pub.aborted || pub.verified === false) n.classes.push('is-stopped');
        if (phase === 'settle' && !pub.aborted) n.classes.push('is-active');
      } else n.sub = t('zWait');
    } else if (n.type === 'ledger'){
      n.title = t('gLedger');
      const root = phase === 'settle' ? (V.phase_fields || {}).state_root : settlement && settlement.state_root;
      if (settleStatus){
        n.sub = (settleStatus === 'settled' ? t('lSettled') : settleStatus === 'released' ? t('lReleased') : settleStatus === 'queued' ? t('lQueued') : t('lCover')) + (root ? ' · ' + shortRoot(root) : '');
      } else n.sub = t('lWait');
      if (phase === 'settle') n.classes.push('is-active');
    }

    const portfolio = n.type === 'taker'
      ? (kind === 'taker' ? taker && taker.portfolio : kind === 'observer' ? obs && obs.taker_portfolio : null)
      : n.type === 'maker'
        ? (kind === 'maker' && isMe ? maker && maker.portfolio
          : kind === 'observer' ? ((obs && obs.maker_portfolios || []).find(p => p.maker === n.index) || {}).portfolio : null)
        : null;
    if (portfolio){
      const asset = (portfolio.inventory || [])[0];
      // Short labels in both languages: the card cell is 40–60px wide. The
      // full wording stays in the tooltip / accessible name.
      n.metrics = [
        {label: lang === 'ja' ? '利用可 資金' : 'cash',
          fullLabel: t('pfAvailable') + ' ' + t('pfCash'), value: showGraphCash(portfolio.cash.available),
          fullValue: showCash(portfolio.cash.available)},
        {label: lang === 'ja' ? '予約 資金' : 'reserved',
          fullLabel: t('pfReserved') + ' ' + t('pfCash'), value: showGraphCash(portfolio.cash.reserved),
          fullValue: showCash(portfolio.cash.reserved), tone: 'reserved'},
      ];
      if (asset){
        n.metrics.push({label: asset.name,
          fullLabel: t('pfAvailable') + ' ' + asset.name, value: showCash(asset.available)});
        // The two-column metric grid has no room for "予約 USD/JPY"; the
        // asset is named by the neighbouring cell and by the tooltip.
        n.metrics.push({label: lang === 'ja' ? '予約分' : t('pfReserved'),
          fullLabel: t('pfReserved') + ' ' + asset.name, value: showCash(asset.reserved), tone: 'reserved'});
      }
    } else if (n.type === 'node'){
      n.metrics = [
        {label: lang === 'ja' ? '保有' : 'custody', fullLabel: lang === 'ja' ? '保有資産' : 'custody',
          value: lang === 'ja' ? '資産なし' : 'none', tone: 'muted'},
        {label: lang === 'ja' ? '受け取り' : 'input', fullLabel: lang === 'ja' ? '受け取るデータ' : 'input',
          value: lang === 'ja' ? '分割した値' : 'shares', fullValue: lang === 'ja' ? '分割した値（秘密分散）' : 'shares', tone: 'muted'},
      ];
    } else if (n.type === 'matcher'){
      // Real configuration only: the node count comes from the room, and the
      // engine name says whether the comparison ran under MPC or in the clear.
      n.metrics = [
        {label: lang === 'ja' ? '計算ノード' : 'nodes', value: lang === 'ja' ? `${nNodes} 台` : String(nNodes)},
        {label: lang === 'ja' ? '比較の方式' : 'engine', value: cfg.engine === 'mpc' ? (lang === 'ja' ? '秘密計算' : 'MPC') : (lang === 'ja' ? '試作（平文）' : 'sim (clear)')},
      ];
    } else if (n.type === 'zkpi'){
      n.metrics = [
        {label: lang === 'ja' ? '検証' : 'proof check', value: pub.verified === true ? (lang === 'ja' ? '合格' : 'passed') : pub.verified === false ? (lang === 'ja' ? '不合格' : 'failed') : '--'},
        {label: lang === 'ja' ? '追加署名' : 'post signature', value: lang === 'ja' ? '不要' : 'none'},
      ];
    } else if (n.type === 'ledger'){
      const root = phase === 'settle' ? (V.phase_fields || {}).state_root : settlement && settlement.state_root;
      n.metrics = [
        {label: lang === 'ja' ? '決済' : 'settlement', value: ledgerOutcome(settleStatus)},
        {label: lang === 'ja' ? '台帳の識別値' : 'ledger hash', value: root ? shortRoot(root) : '--'},
      ];
    }
    if (hasExecutionProgress()) {
      const statusLabel = lang === 'ja' ? '状況' : 'status';
      const timeLabel = lang === 'ja' ? '経過' : 'elapsed';
      if (n.type === 'node') {
        const current = (P.nodes || []).find(entry => entry.node === n.index);
        const state = current ? current.status : 'waiting';
        n.sub = executionLabel(current ? current.step : 'waiting');
        n.classes = n.classes.filter(cls => !['is-active','is-stopped'].includes(cls));
        if (state === 'running' && P.active) n.classes.push('is-active');
        if (state === 'failed' || state === 'interrupted') n.classes.push('is-stopped');
        n.metrics = [
          {label:statusLabel, value:executionState(state)},
          {label:timeLabel, value:current && current.step !== 'waiting' ? formatDuration(current.elapsed_ms, lang) : '—'},
        ];
      } else {
        const targets = {
          sending:['taker'], policy:['ledger'], reserve:['taker'],
          admission:['matcher'], mpc:['matcher'], proof:['zkpi'],
          settlement:['ledger'], reconcile:['ledger','taker'], display:['matcher'],
        };
        const current = (targets[P.stage] || []).includes(n.type);
        if (current) {
          n.sub = executionLabel(P.stage);
          n.classes = n.classes.filter(cls => !['is-active','is-stopped'].includes(cls));
          if (P.active) n.classes.push('is-active');
          if (P.status === 'failed') n.classes.push('is-stopped');
          // Custody cards retain their real available/reserved balances.
          if (['matcher','zkpi','ledger'].includes(n.type)) n.metrics = [
            {label:statusLabel, value:P.status === 'done' ? (lang === 'ja' ? '完了' : 'Complete') : executionState(P.status)},
            {label:timeLabel, value:formatDuration(P.stage_ms || 0, lang)},
          ];
        } else if (P.active && ['matcher','zkpi','ledger'].includes(n.type)) {
          const completed = ({
            matcher:['proof','settlement','reconcile'],
            zkpi:['settlement','reconcile'],
          }[n.type] || []).includes(P.stage);
          n.sub = completed ? (lang === 'ja' ? 'この工程は完了' : 'Stage complete')
            : (lang === 'ja' ? '前の工程を待っています' : 'Waiting for earlier stages');
          n.classes = n.classes.filter(cls => cls !== 'is-active');
          n.metrics = [{label:statusLabel, value:completed ? (lang === 'ja' ? '完了' : 'Complete') : executionState('waiting')}];
        }
      }
    }
  });
  if (hasExecutionProgress()) {
    // Use the real execution stage, never the later phase replay. Blue flows
    // show the operation in progress; Gateway RPC edges show observed calls.
    edges.forEach(edge => { edge.state = 'idle'; edge.particle = false; });
    labels.length = 0;
    if (P.active) {
      const stageLinks = {
        mpc: edges.filter(edge => edge.target === 'matcher').map(edge => edge.id),
        proof: [edgeId('matcher', 'zkpi')],
        settlement: [edgeId('zkpi', 'ledger')],
        reconcile: edges.filter(edge => edge.source === 'ledger').map(edge => edge.id),
      };
      for (const edge of edges) {
        if ((stageLinks[P.stage] || []).includes(edge.id)) {
          edge.state = 'flow'; edge.particle = true; edge.own = true; edge.color = 'blue';
        }
      }
      if (['policy','reserve'].includes(P.stage)) {
        edges.push({id:'stage-gateway-ledger',source:'gateway',target:'ledger',
          state:'flow',own:true,particle:true,color:'blue',
          d:orthoPath([right(gateway),{x:laneR,y:gateway.y},{x:laneR,y:ledger.y},right(ledger)])});
      } else if (['sending','admission'].includes(P.stage)) {
        edges.push({id:'stage-taker-gateway',source:'taker',target:'gateway',
          state:'flow',own:true,particle:true,color:'blue',
          d:orthoPath([bottom(takerN),{x:takerN.x,y:gateway.y},left(gateway)])});
      }
    }
    const events = P.events || [];
    for (let i = 0; i < nNodes; i++) {
      const current = (P.nodes || []).find(entry => entry.node === i);
      const latest = [...events].reverse().find(event => event.node === i);
      if (!latest) continue;
      const running = P.active && current && current.status === 'running';
      const isReply = ['received','rejected'].includes(latest.status);
      const recentReply = isReply && P.active && P.elapsed_ms - latest.at_ms < 1500;
      const from = running || !isReply ? gateway : byId['node:' + i];
      const to = from === gateway ? byId['node:' + i] : gateway;
      edges.push({id:'rpc-' + i, source:from.id, target:to.id,
        state:running || recentReply ? 'flow' : latest.status === 'failed' ? 'cut' : 'done',
        own:true, particle:running || recentReply, color:running || latest.status === 'rejected' ? 'amber' : 'teal',
        d:orthoPath([from === gateway ? bottom(from) : top(from),
          {x:from.x,y:participantBottom+126}, {x:to.x,y:participantBottom+126},
          to === gateway ? bottom(to) : top(to)])});
    }
  }
  return {nodes, edges, labels, W, H, compact};
}

function drawNetworkGraph(){
  const container = $('network-graph');
  if (!container || !V) return;
  // Use the real canvas width. graphModel has a dedicated narrow layout; the
  // old 980px floor forced a desktop graph to be miniaturised inside phones.
  // The container is the viewport under the top bar, minus the drawer while
  // one is open on a desktop, so the diagram is laid out for what is visible.
  const W = container.clientWidth > 0 ? Math.max(320, container.clientWidth) : 980;
  lastGraphWidth = container.clientWidth;
  const model = graphModel(W);
  lastModel = model;
  container.setAttribute('aria-label', t('graphAria'));
  const noRound = !(V.public && V.public.number) && V.phase === 'idle';
  if (!window.QommNetworkGraph || typeof window.QommNetworkGraph.render !== 'function'){
    container.classList.add('graph-fallback');
    container.textContent = lang === 'ja' ? 'ネットワーク図を読み込めませんでした。' : 'The network graph could not be loaded.';
    return;
  }
  // On a phone the bottom sheet covers the lower part of the canvas; the
  // bundle pans the selected card into the strip that stays visible.
  lastInset = model.compact && drawerPane ? (Number($('drawer').offsetHeight) || 0) : 0;
  window.QommNetworkGraph.render(container, model, {
    ariaLabel: t('graphAria'),
    phase: V.phase,
    phaseLabel: phaseCaption(),
    noRoundText: noRound ? t('noRoundYet') + ' ' + phaseCaption() : '',
    legend: [['taker','gTaker'],['maker','gMaker'],['node','gNode'],['matcher','gMatcher'],['zkpi','gZkpi'],['ledger','gLedger']]
      .map(([type, key]) => ({type, label: t(key)})),
    legendNotes: legendNotesList(),
    reducedMotion,
    compact: !!model.compact,
    selectedId: isCardPane(drawerPane) ? drawerPane : null,
    insetBottom: lastInset,
    selectHint: t('selectHint'),
    onNodeSelect: onGraphNodeSelect,
  });
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
  drawAssurance();
  if (!V) return;
  $('countdown').textContent =
    (V.config.auto_rounds && V.next_round_in !== null && V.next_round_in !== undefined)
      ? '⟳ ' + V.next_round_in.toFixed(0) + 's' : '';
  const signature = JSON.stringify(V, (k, v) => k === 'next_round_in' ? 0 : v) + JSON.stringify(P);
  if (signature === lastDraw) return;
  lastDraw = signature;
  document.documentElement.lang = lang;
  applyStrings();
  const c = V.config;
  // Engine notice on the diagram, not only the header badge: what the room
  // actually guarantees right now, before any design description.
  drawEngineNotice(c);
  const eng = $('engineBadge');
  eng.textContent = c.engine === 'sim' ? t('engineSim') : t('engineMpc');
  eng.dataset.short = c.engine === 'sim' ? t('engineSimShort') : t('engineMpcShort');
  eng.title = (c.engine === 'sim' ? t('engineSimNote') : t('engineMpcNote'))
    + (lang === 'en' && c.engine_note ? ' · ' + c.engine_note : '');
  eng.className = 'badge engine' + (c.engine === 'sim' ? '' : ' mpc');
  $('roundNum').textContent = (V.public && V.public.number)
    ? '#' + V.public.number + ' · ' + (lang === 'ja' ? '処理時間 ' : '')
      + formatDuration(V.public.ms, lang) : '#--';
  // The diagram is on screen from the first view, seat or no seat: a seatless
  // browser sees the public projection and joins from a seat card.
  $('stage').classList.remove('hide');
  seatTransition();
  drawTopbar();
  layoutDrawer();
  const seatsNow = JSON.stringify(V.seats) + ':' + lang + ':' + (c.engine || '');
  if (seatsNow !== seatsSig){ seatsSig = seatsNow; buildLobby(); }
  drawPhaseStrip();
  drawNetworkGraph();
  drawPortfolioStrip();
  drawTransferSummary();
  drawChat();
  const want = V.kind + ':' + V.index + ':' + lang;
  if (built !== want){ buildPanel(); built = want; }
  updatePanel();
  drawPublic();
  drawHistory();
  drawSeatMap();
  drawNotices();
  drawLegend();
  // The balance strip and the chat decide their own visibility from the seat;
  // the pane decides whether they are on screen at all.
  layoutDrawer();
  fillDrawer();
  drawHint();
  // A phone's bottom sheet is only as tall as its content; the diagram keeps
  // the selected card above it, so redraw once that height is known.
  if (isCompact() && drawerPane && (Number($('drawer').offsetHeight) || 0) !== lastInset) drawNetworkGraph();
}

function drawPublic(){
  const p = V.public || {}, box = $('publicbody');
  const queuePending = isQueuePending(p);
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
    rows.push([t('protocolMs'), formatDuration(st.protocol_ms, lang)],
              [t('engRounds'), st.rounds], [t('engMb'), st.mb],
              [t('compiledOnce'), formatDuration(st.compiled_once_ms, lang)]);
  }
  rows.forEach(([a, b]) => { const tr = el('tr');
    tr.appendChild(el('td', null, a)); tr.appendChild(el('td', 'mono', String(b)));
    table.appendChild(tr); });
  box.appendChild(table);
  if (p.settlement){
    const settlement = el('div', 'hidden-box stack'); settlement.style.marginTop = '.5rem';
    settlement.appendChild(el('div', 'tag brand', t('gLedger')));
    settlement.appendChild(el('span', 'status-pill ' + p.settlement.status, settlementName(p.settlement.status)));
    settlement.appendChild(el('div', 'root', p.settlement.state_root || '--'));
    box.appendChild(settlement);
  }
  if (queuePending){
    const waiting = el('div', 'hidden-box stack');
    waiting.style.marginTop = '.5rem';
    waiting.appendChild(el('div', 'tag', t('queued')));
    waiting.appendChild(el('div', null, abortText(p)));
    box.appendChild(waiting);
  } else if (p.verified !== null && p.verified !== undefined){
    const v = el('div', p.verified ? 'hidden-box' : 'bad-box');
    v.style.marginTop = '.5rem';
    v.appendChild(el('div', p.verified ? 'tag' : 'tag bad', p.verified ? t('verifiedYes') : t('verifiedNo')));
    v.appendChild(el('div', 'why mono', verifiedDetailText(p.verified_detail)));
    box.appendChild(v);
  }
  if (p.aborted && !queuePending){ const bad = el('div', 'bad-box');
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
    if (isQueuePending(h)) row.appendChild(el('span', 'warn', t('queued') + ' — ' + abortText(h)));
    else if (h.aborted) row.appendChild(el('span', 'bad', t('stopped') + ' — ' + abortText(h)));
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
  // The newest notice also sits on the diagram: a maker learns about a fill
  // without opening any pane.
  const latest = $('notice-latest');
  if (latest){
    if (rows.length){ latest.textContent = msg(rows[0].code, rows[0].fields); latest.className = 'notice-latest ' + (rows[0].tone || 'info'); }
    else latest.className = 'notice-latest hide';
  }
  if (!rows.length){ box.appendChild(el('p', 'empty', '--')); return; }
  rows.forEach(n => box.appendChild(el('div', n.tone, msg(n.code, n.fields))));
}

/* ========================================================================= */
/*  Graph-first frame: top bar, status overlay, legend, drawer                */
/* ========================================================================= */
function applyStrings(){
  document.querySelectorAll('[data-s]').forEach(n => {
    const v = t(n.dataset.s); if (typeof v === 'string') n.textContent = v; });
  document.querySelectorAll('[data-s-placeholder]').forEach(n => {
    const v = t(n.dataset.sPlaceholder); if (typeof v === 'string') n.placeholder = v; });
  document.querySelectorAll('[data-s-aria]').forEach(n => {
    const v = t(n.dataset.sAria); if (typeof v === 'string') n.setAttribute('aria-label', v); });
  $('btn-lang').textContent = t('langOther');
}
function show(id, on){ const n = $(id); if (n) n.classList.toggle('hide', !on); }
function isCompact(){ const g = $('network-graph'); return !!g && g.clientWidth > 0 && g.clientWidth < 560; }
function isCardPane(pane){ return !!pane && pane !== 'seats' && pane !== 'me'; }
/* The pane that holds this browser's own controls: the seat's card, or 'me'
   for the observer, who has no card in the diagram. */
function myPaneId(){
  if (!V || !V.seat) return null;
  if (V.kind === 'observer') return 'me';
  return V.kind === 'taker' ? 'taker' : V.kind + ':' + V.index;
}
function legendNotesList(){
  return (isSim() ? [t('legendSim')] : []).concat([t('legendNoCustody'), t('legendLines'), t('productBoundary'), ledgerNote()]);
}
function describeOrder(o){
  return o ? `${(V.assets[o.asset] || {}).name || '--'} ${o.direction ? t('sell') : t('buy')} ${o.qty}${o.is_real ? '' : ' · ' + t('cover')}` : '';
}
function resetSeatState(){
  built = ''; chatHistory = []; chatPending = null;
  pfPrev = null; pfPrevRound = null; pfRoundBaseline = null; pfDelta = {}; pfDeltaKind = '';
}
/* Seatless → seated opens the seat's own controls; seated → seatless returns
   to the seat picker; a switch between seats follows the new seat. Anything
   the person opened or closed themselves stays until the seat changes. */
function seatTransition(){
  const seatId = V.seat || null;
  if (drawerPane === undefined) drawerPane = seatId ? myPaneId() : V.completed_taker ? 'taker' : 'seats';
  else if (seatId !== lastSeatId){
    if (lastSeatId !== null) resetSeatState();
    const readingHistory = !seatId && V.completed_taker && drawerPane === 'gateway' && selectedExecution !== null;
    if (!readingHistory) drawerPane = seatId ? myPaneId() : V.completed_taker ? 'taker' : 'seats';
    hintClosed = false;
  }
  lastSeatId = seatId;
}
let detailsHidden = false;
if (hasDom) { try { detailsHidden = localStorage.getItem('qomm.detailsHidden') === '1'; } catch (_) {} }
function drawDetailsToggle(){
  document.body.classList.toggle('details-hidden', detailsHidden);
  const button = $('btn-details');
  button.textContent = lang === 'ja' ? (detailsHidden ? '説明を表示' : '説明を隠す') : (detailsHidden ? 'Show details' : 'Hide details');
  button.setAttribute('aria-expanded', String(!detailsHidden));
}
function drawTopbar(){
  drawDetailsToggle();
  const badge = $('seatBadge');
  const seatLabel = V.kind === 'taker' ? t('seatTaker') : V.kind === 'maker' ? t('seatMaker') + ' ' + V.index
    : V.kind === 'node' ? t('seatNode') + ' ' + V.index : V.kind === 'observer' ? t('seatObserver') : '';
  badge.textContent = seatLabel + (V.label ? ' · ' + V.label : '');
  badge.classList.toggle('hide', !V.seat);
  $('view-current').textContent = V.seat ? '' : t('viewPublic');
  $('btn-view').title = t('viewHelp');
  $('btn-leave').classList.toggle('hide', !V.seat);
  const me = $('btn-me');
  me.classList.toggle('hide', !V.seat);
  me.textContent = V.kind === 'observer' ? t('settings') : t('myControls');
}
/* One line on the diagram saying what the engine guarantees; the simulation's
   longer explanation unfolds on request. */
function drawEngineNotice(c){
  const banner = $('engine-banner');
  const sim = c.engine === 'sim';
  $('engine-short').textContent = sim ? t('engineNoticeSimShort') : t('engineNoticeMpc');
  $('engine-full').textContent = sim ? t('engineNoticeSim') : '';
  $('engine-more').classList.toggle('hide', !sim);
  banner.classList.toggle('sim', sim);
  banner.classList.remove('hide');
}
function drawLegend(){
  const chips = $('legend-chips'); chips.textContent = '';
  [['taker','gTaker'],['maker','gMaker'],['node','gNode'],['matcher','gMatcher'],['zkpi','gZkpi'],['ledger','gLedger']]
    .forEach(([type, key]) => chips.appendChild(el('span', 'legend-chip ' + type, t(key))));
  const notes = $('legend-notes'); notes.textContent = '';
  legendNotesList().forEach((note, i) => notes.appendChild(el('div', isSim() && i === 0 ? 'legend-note sim' : 'legend-note', note)));
}
/* Where to click, for someone who has not clicked yet. */
function drawHint(){
  const box = $('stage-hint'); if (!box || !V) return;
  let text = '';
  if (!hintClosed){
    if (!V.seat){ if (drawerPane !== 'seats') text = t('hintSeatless'); }
    else if (!guided) text = t(V.kind === 'observer' ? 'hintObserver' : 'hintSeated') + (isCompact() ? '' : ' ' + t('hintKeys'));
  }
  $('stage-hint-text').textContent = text;
  box.classList.toggle('hide', !text);
}
function markGuided(){
  guided = true;
  try { localStorage.setItem('qomm.guided', '1'); } catch (e) { /* private mode */ }
}
function openPane(pane){
  drawerPane = pane;
  if (isCardPane(pane)) markGuided();
  drawDrawer(); drawHint(); drawNetworkGraph();
  $('drawer-body').scrollTop = 0;
  const title = $('drawer-title'); if (title && drawerPane) title.focus();
}
function closeDrawer(){
  const was = drawerPane;
  drawerPane = null;
  drawDrawer(); drawHint(); drawNetworkGraph();
  const back = isCardPane(was) ? findNodeCard(was) : null;
  (back || $(was === 'me' ? 'btn-me' : 'btn-view')).focus();
}
function findNodeCard(id){
  const cards = document.querySelectorAll('#network-graph [data-node-id]');
  for (const card of cards){ if (card.getAttribute('data-node-id') === id) return card; }
  return null;
}
function onGraphNodeSelect(id){ if (V) openPane(id); }
function setDrawerHeading(kicker, title, sub){
  $('drawer-kicker').textContent = kicker || '';
  $('drawer-title').textContent = title || '';
  $('drawer-sub').textContent = sub || '';
}
function paneSeat(pane){ return isCardPane(pane) ? (V.seats || []).find(s => s.id === pane) || null : null; }
/* Which of the static slots are on screen for the current pane. The slots
   themselves are drawn on every view whether visible or not, so a pane opens
   already filled. */
function layoutDrawer(){
  if (!V) return;
  const pane = drawerPane || null;
  const open = !!pane;
  const mine = myPaneId();
  const isSeats = pane === 'seats';
  const isMine = open && pane === mine;
  const card = isCardPane(pane) ? pane : null;
  const seat = paneSeat(pane);
  const process = card && !seat ? card : null;
  const chatty = V.kind === 'taker' || V.kind === 'maker';
  $('drawer').classList.toggle('hide', !open);
  $('network-graph').classList.toggle('with-drawer', open);
  $('btn-view').setAttribute('aria-expanded', isSeats ? 'true' : 'false');
  $('btn-me').setAttribute('aria-expanded', isMine ? 'true' : 'false');
  show('lobby', isSeats);
  show('pane-seats', isSeats);
  show('node-info', !!card);
  show('panel', isMine);
  show('portfolio-strip', isMine);
  show('chat-panel', isMine && chatty);
  show('pane-public', process === 'matcher');
  show('pane-history', process === 'matcher' || process === 'ledger');
}
function fillDrawer(){
  if (!V) return;
  const pane = drawerPane || null;
  if (!pane) return;
  if (pane === 'seats') setDrawerHeading(t('paneJoinKicker'), V.seat ? t('paneSwitchTitle') : t('lobbyTitle'), V.seat ? t('paneSwitchWhy') : '');
  else if (pane === 'me') setDrawerHeading(t('observerTitle'), t('paneObserverTitle'), t('observerWhy'));
  else drawNodeInfo(pane, paneSeat(pane));
}
function drawDrawer(){ layoutDrawer(); fillDrawer(); }
function nodeHeading(id){
  const n = lastModel && lastModel.nodes.find(x => x.id === id);
  return n ? {title: n.title, sub: n.sub || ''} : {title: id, sub: ''};
}
function roleWhy(kind){
  return t(kind === 'taker' ? 'roleTakerWhy' : kind === 'maker' ? 'roleMakerWhy' : engineKey('roleNodeWhy'));
}
/* The selected card, explained for this seat: who holds it and how to take
   it, or what the process is doing. What a seat may see is decided by the
   view the server sent; nothing here reaches past it. */
function drawNodeInfo(id, seat){
  const box = $('node-info');
  // Do not rebuild under someone typing a name into the claim form.
  const active = document.activeElement;
  if (active && active.classList.contains('claim-name') && seat && !seat.mine && !seat.held && typeof box.contains === 'function' && box.contains(active)) return;
  box.textContent = '';
  const {title, sub} = nodeHeading(id);
  if (seat){
    const kicker = seat.mine ? t('paneMine') : seat.kind === 'taker' ? t('roleTaker') : seat.kind === 'maker' ? t('roleMaker') : t('roleNode');
    setDrawerHeading(kicker, title, sub);
    if (seat.mine){
      box.appendChild(el('p', 'why', roleWhy(seat.kind)));
    } else {
      box.appendChild(el('p', 'pane-lead', seat.held ? tf('seatHeldBy', {who: seat.label || t('taken')}) : t('seatAuto')));
      box.appendChild(el('p', 'why', roleWhy(seat.kind)));
      if (!seat.held) box.appendChild(claimForm(seat));
      box.appendChild(el('p', 'why', t('seatPrivate')));
    }
    if (id === 'taker' && V.completed_taker) {
      const receipt = V.completed_taker, last = receipt.last || {};
      box.prepend(el('p', 'pane-lead', lang === 'ja' ? '注文が完了し、席を自動で空けました。' : 'Order complete. Your seat was released automatically.'));
      box.appendChild(el('h3', '', (lang === 'ja' ? 'あなたの注文結果 #' : 'Your order #') + last.number));
      box.appendChild(el('p', '', `${(V.assets[last.asset] || {}).name || ''} · ${last.qty} · ${showPrice(V.assets, last.asset, last.price)}`));
      const result = el('div'); drawSettlement(result, receipt.settlement); box.appendChild(result);
      box.appendChild(el('p', '', (lang === 'ja' ? '完了時の利用可能資金: ' : 'Available cash at completion: ') + showCash(receipt.portfolio.cash.available)));
    }
    if (V.kind === 'observer') box.appendChild(observerSeatDetail(seat));
    return;
  }
  setDrawerHeading(t('paneProcess'), title, sub);
  if (id === 'gateway'){
    drawTransferHistory(box);
  } else if (id === 'matcher'){
    box.appendChild(el('p', 'why', t(engineKey('paneMatcherWhy'))));
    if (V.kind === 'observer') observerQuotes(box);
  } else if (id === 'zkpi'){
    box.appendChild(el('p', 'why', t('paneZkpiWhy')));
    verificationDetail(box);
  } else if (id === 'ledger'){
    box.appendChild(el('p', 'why', t('paneLedgerWhy')));
    ledgerDetail(box);
  }
}
function claimForm(seat){
  const form = el('div', 'claim-form');
  const label = el('label', 'field-label', t('yourName'));
  const input = el('input', 'claim-name'); input.type = 'text'; input.maxLength = 24;
  input.value = $('pname').value;
  input.setAttribute('aria-label', t('yourName'));
  input.oninput = () => { $('pname').value = input.value; };
  const go = el('button', 'go', V.seat ? t('claimSwitch') : t('claimHere')); go.type = 'button';
  go.onclick = () => send({type: 'claim', seat: seat.id, label: input.value});
  form.appendChild(label); form.appendChild(input); form.appendChild(go);
  return form;
}
function kv(pairs){
  const list = el('dl', 'kv');
  pairs.forEach(([k, v]) => { list.appendChild(el('dt', null, k)); list.appendChild(el('dd', null, v)); });
  return list;
}
function balancesList(p){
  const rows = [[t('pfCash') + ' · ' + t('pfAvailable'), showCash(p.cash.available)],
                [t('pfCash') + ' · ' + t('pfReserved'), showCash(p.cash.reserved)]];
  (p.inventory || []).forEach(r => {
    rows.push([r.name + ' · ' + t('pfAvailable'), showCash(r.available)]);
    if (r.reserved) rows.push([r.name + ' · ' + t('pfReserved'), showCash(r.reserved)]);
  });
  const box = el('div');
  box.appendChild(el('div', 'tag', t('obsBalances')));
  box.appendChild(kv(rows));
  return box;
}
/* The observer's view of one seat: demo-only, and labelled as such so the
   full view is not mistaken for what a participant can see. */
function observerSeatDetail(seat){
  const d = V.observer || {};
  const wrap = el('div', 'observer-detail');
  wrap.appendChild(el('div', 'tag leak', t('observerTitle')));
  wrap.appendChild(el('p', 'why', t('observerSeatWhy')));
  if (seat.kind === 'taker'){
    if (d.request){
      const settled = !!((d.settlements || [])[0] && d.settlements[0].status === 'settled');
      wrap.appendChild(kv([
        [t('request'), describeOrder(d.request)],
        [settled ? t('winner') : t('candidate'), d.winner === null || d.winner === undefined ? '--' : t('gMaker') + ' ' + d.winner],
        [t('price'), showPrice(V.assets, d.request.asset, d.price)],
      ]));
    } else wrap.appendChild(el('p', 'empty', t('noRoundYet')));
    if (d.taker_portfolio) wrap.appendChild(balancesList(d.taker_portfolio));
  } else if (seat.kind === 'maker'){
    const pol = (d.policies || [])[seat.index];
    if (pol){
      wrap.appendChild(el('div', 'tag', t('obsPolicy')));
      wrap.appendChild(kv([[t('assetLabel'), (V.assets[pol.asset] || {}).name || '--']]
        .concat(Object.keys(POLICY_RANGE).map(name => [t(name), String(pol[name])]))
        .concat([[t('active'), pol.active ? t('on') : t('off')]])));
    }
    const q = (d.quotes || []).find(x => x.maker === seat.index);
    if (q){
      const shownIn = d.request ? d.request.asset : q.asset;
      wrap.appendChild(el('div', 'tag', t('obsQuote')));
      wrap.appendChild(kv([[t('ask'), showPrice(V.assets, shownIn, q.ask)], [t('bid'), showPrice(V.assets, shownIn, q.bid)],
        [t('reason'), showQuoteReason(q.reason) || (q.eligible ? t('eligibleShort') : '--')]]));
    }
    const p = (d.maker_portfolios || []).find(x => x.maker === seat.index);
    if (p && p.portfolio) wrap.appendChild(balancesList(p.portfolio));
  } else if (seat.kind === 'node'){
    const b = (d.behaviours || {})[seat.index];
    wrap.appendChild(kv([[t('obsBehaviour'), b ? t('b_' + b) : '--']]));
  }
  return wrap;
}
function verificationDetail(box){
  const p = V.public || {};
  if (!p.number){ box.appendChild(el('p', 'empty', t('noRoundYet'))); return; }
  if (isQueuePending(p)){
    const w = el('div', 'hidden-box'); w.appendChild(el('div', 'tag', t('queued')));
    w.appendChild(el('div', null, abortText(p))); box.appendChild(w);
  } else if (p.verified === true || p.verified === false){
    const v = el('div', p.verified ? 'hidden-box' : 'bad-box');
    v.appendChild(el('div', p.verified ? 'tag' : 'tag bad', p.verified ? t('verifiedYes') : t('verifiedNo')));
    v.appendChild(el('div', 'why mono', verifiedDetailText(p.verified_detail)));
    box.appendChild(v);
  } else if (p.aborted){
    const b = el('div', 'bad-box'); b.appendChild(el('div', 'tag bad', t('stopped')));
    b.appendChild(el('div', null, abortText(p))); b.appendChild(el('div', 'why', p.abort_reason));
    box.appendChild(b);
  } else {
    const ok = el('div', 'hidden-box'); ok.appendChild(el('div', 'tag', t('zDecoded')));
    ok.appendChild(el('div', 'why', t('corrected') + ': ' + (p.corrections || 0) + ' / ' + (p.reductions || 0)));
    box.appendChild(ok);
  }
  if (p.settlement && p.settlement.automatic){
    const auto = el('div', 'hidden-box', t('automatic')); auto.style.marginTop = '.5rem'; box.appendChild(auto);
  }
  const tech = el('details', 'pane-details'); tech.open = techOpen;
  tech.ontoggle = () => { techOpen = !!tech.open; };
  tech.appendChild(el('summary', null, t('techNotes')));
  tech.appendChild(el('p', 'why', t('zkpiTech')));
  tech.appendChild(el('p', 'why', t('productBoundary')));
  tech.appendChild(el('p', 'why', ledgerNote()));
  if (lang === 'en' && V.config.engine_note) tech.appendChild(el('p', 'why', V.config.engine_note));
  box.appendChild(tech);
}
/* The ledger card: this seat's own settlement when the server sent one, the
   public record otherwise, and where that record actually lives. */
function ledgerDetail(box){
  const own = V.kind === 'taker' ? (V.taker && V.taker.settlement) : V.kind === 'maker' ? (V.maker && V.maker.settlement)
    : V.kind === 'observer' ? ((V.observer && V.observer.settlements) || [])[0] : null;
  const pub = (V.public && V.public.settlement) || null;
  const wrap = el('div');
  if (own) drawSettlement(wrap, own);
  else if (pub){
    wrap.appendChild(el('div', 'tag brand', t('publicRecord')));
    wrap.appendChild(el('span', 'status-pill ' + pub.status, settlementName(pub.status)));
    if (pub.state_root){
      wrap.appendChild(el('div', 'tag', t(pub.status === 'queued' ? 'queueStateRoot' : 'stateRoot')));
      wrap.appendChild(el('div', 'root', pub.state_root));
    }
  } else wrap.appendChild(el('p', 'empty', t('noSettlement')));
  box.appendChild(wrap);
  const note = el('p', 'why', ledgerNote()); note.style.marginTop = '.5rem';
  box.appendChild(note);
}

/* ─── lobby ────────────────────────────────────────────────────────── */
function buildLobby(){
  const grid = $('lobby-grid'); grid.textContent = ''; grid.setAttribute('aria-busy', 'false');
  // Seats are grouped by what the role does, in the order a newcomer reads
  // the market: who asks for a trade, who quotes, who computes.
  const groups = [['taker', 'roleTaker', 'roleTakerWhy'], ['maker', 'roleMaker', 'roleMakerWhy'], ['node', 'roleNode', 'roleNodeWhy']];
  groups.forEach(([kind, titleKey, whyKey]) => {
    const seats = V.seats.filter(s => s.kind === kind);
    if (!seats.length) return;
    const head = el('div', 'lobby-group');
    head.appendChild(el('b', null, t(titleKey)));
    head.appendChild(el('span', null, t(engineKey(whyKey))));
    grid.appendChild(head);
    seats.forEach(s => {
      const b = el('button'); b.type = 'button';
      const name = s.kind === 'taker' ? t('seatTaker') : (s.kind === 'maker' ? t('seatMaker') : t('seatNode')) + ' ' + s.index;
      b.appendChild(el('b', null, name));
      b.appendChild(el('span', null, s.mine ? t('you') + (s.label ? ' · ' + s.label : '')
        : s.held ? (s.label || t('taken')) : t('auto')));
      if (s.mine) b.classList.add('mine');
      b.disabled = s.held;
      b.onclick = () => send({type:'claim', seat:s.id, label:$('pname').value});
      grid.appendChild(b);
    });
  });
  const ex = $('explain'); ex.textContent = '';
  if (isSim()){
    const note = el('div', 'note-box'); note.style.marginBottom = '.6rem';
    note.appendChild(el('b', null, t('designNote') + ' — '));
    note.appendChild(document.createTextNode(t('engineNoticeSim')));
    ex.appendChild(note);
  }
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
  // This text names a button group rather than one form control.  A <label>
  // without a matching input makes browsers report an inaccessible field;
  // the group itself carries the accessible name through aria-label.
  if (label) row.appendChild(el('span', 'field-label', label));
  const seg = el('div', 'seg'); seg.id = id; seg.setAttribute('role', 'group');
  if (label) seg.setAttribute('aria-label', label);
  options.forEach(o => { const b = el('button', o.tone || '', o.text);
    b.type = 'button'; b.dataset.value = o.value;
    b.setAttribute('aria-pressed', 'false');
    b.onclick = () => { pick(seg, o.value); onPick(o.value); }; seg.appendChild(b); });
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
  limit.min = '0'; limit.step = '0.01'; limit.inputMode = 'decimal';
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
  go.onclick = () => {
    const selected = Number(sel.value);
    const selectedButton = (group, fallback) => {
      const button = group && group.querySelector('button[aria-pressed="true"]');
      return button ? Number(button.dataset.value) : fallback;
    };
    const scale = assetScale(V.assets, selected);
    const pending = (V.taker && V.taker.pending) || {};
    const values = {
      asset: selected,
      direction: selectedButton(side, Number(pending.direction || 0)),
      qty: Math.max(1, Math.round(Number(qty.value))),
      limit_price: Math.max(1, Math.round(Number(limit.value) * scale)),
      is_real: selectedButton(kind, Number(pending.is_real === 0 ? 0 : 1)),
    };
    go.disabled = true;
    go.textContent = t('waiting');
    send({type:'submit', values});
  };
  c.appendChild(go);
  root.appendChild(c);

  const r = card(t('openedTitle'), t('openedWhy'));
  const opened = el('div', 'leak-box');
  opened.appendChild(el('div', 'tag leak', t('everyoneSees')));
  const maskedKey = el('div', 'opaque', '--');
  opened.appendChild(maskedKey); r.appendChild(opened);
  r.appendChild(el('div', 'why', '↓ ' + t('minusMask')));
  const priv = el('div', 'hidden-box');
  const priceTag = el('div', 'tag', t('yourPrice')); priv.appendChild(priceTag);
  const price = el('div', 'big', '--'); priv.appendChild(price);
  const winner = el('div', null, ''); priv.appendChild(winner);
  r.appendChild(priv);
  root.appendChild(r);
  const settlementCard = card(t('settlementTitle'), '');
  const settlement = el('div'); settlementCard.appendChild(settlement); root.appendChild(settlementCard);
  takerEls = {sel, side, qty, limitLabel, limit, limitHint, kind, reserve, go, maskedKey, priceTag, price, winner, settlement};
}
function updateTaker(){
  const d = V.taker || {}, p = {...(d.pending || {}), ...takerEdits}, last = d.last, e = takerEls;
  setVal(e.sel, p.asset); pick(e.side, p.direction); pick(e.kind, p.is_real);
  setVal(e.qty, p.qty);
  if (e.qty && e.qty.parentElement) e.qty.parentElement.querySelector('.val').textContent = p.qty;
  const asset = V.assets[p.asset] || {scale:1};
  const digits = String(asset.scale).length - 1;
  const maximumPrice = Math.floor(((2 ** 48) - 1) / Math.max(1, Number(p.qty || 1)));
  e.limit.min = (1 / asset.scale).toFixed(digits);
  e.limit.max = (maximumPrice / asset.scale).toFixed(digits);
  e.limit.step = (1 / asset.scale).toFixed(digits);
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
  if (V.public && isQueuePending(V.public)){
    e.price.textContent = '--'; e.winner.textContent = t('queued') + ' — ' + abortText(V.public);
    return;
  }
  if (V.public && V.public.aborted){
    e.price.textContent = '--'; e.winner.textContent = t('stopped') + ' — ' + abortText(V.public);
    return;
  }
  if (last.winner === null || last.winner === undefined){
    e.price.textContent = '--'; e.winner.textContent = t('noMaker');
    return;
  }
  // A picked quote is only a counterparty once this round settled; a limit
  // miss or a released reserve leaves it a candidate.
  const filled = !!(d.settlement && d.settlement.status === 'settled');
  e.priceTag.textContent = filled ? t('yourPrice') : t('candidatePrice');
  e.price.textContent = showPrice(V.assets, last.asset, last.price);
  e.winner.textContent = (filled ? t('winnerIs') : t('candidate')) + ': ' + t('gMaker') + ' ' + last.winner
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
  if (isSim()) c.appendChild(el('div', 'note-box', t('engineNodeSim')));
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
  if (isQueuePending(p)){ const waiting = el('div', 'hidden-box'); waiting.style.marginTop = '.5rem';
    waiting.appendChild(el('div', 'tag', t('queued')));
    waiting.appendChild(el('div', null, abortText(p)));
    e.verdict.appendChild(waiting); }
  else if (p.aborted){ const bad = el('div', 'bad-box'); bad.style.marginTop = '.5rem';
    bad.appendChild(el('div', null, abortText(p)));
    bad.appendChild(el('div', 'why', p.abort_reason));
    e.verdict.appendChild(bad); }
}

/* ─── observer ─────────────────────────────────────────────────────── */
// The observer's own pane holds the room controls. Everything else it may
// see hangs off the card it belongs to: all quotes on the matcher card,
// settlements on the ledger card, a seat's policy, order or behaviour on that
// seat's card (drawNodeInfo).
let obsEls = {};
function buildObserver(root){
  const cfg = card(t('settings'), t('settingsWhy'));
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

  const b = card(t('behaviours'), t('behavioursWhy'));
  const behav = el('div', 'seatmap'); b.appendChild(behav); root.appendChild(b);
  root.appendChild(el('p', 'why', t('observerPanes')));
  obsEls = {rs, sm, ar, ic, now, behav};
}
function observerQuotes(box){
  const d = V.observer || {};
  const latestSettlement = (d.settlements || [])[0];
  const roundSettled = !!(latestSettlement && latestSettlement.status === 'settled');
  box.appendChild(el('div', 'tag leak', t('observerTitle')));
  const head = el('div', 'row');
  if (d.request){
    const parts = [
      [t('request'), describeOrder(d.request)],
      [roundSettled ? t('winner') : t('candidate'), d.winner === null || d.winner === undefined ? '--' : t('gMaker') + ' ' + d.winner],
      [t('price'), showPrice(V.assets, d.request.asset, d.price)],
    ];
    parts.forEach(([k, v]) => { const item = el('div', 'hidden-box');
      item.style.flex = '1'; item.appendChild(el('div', 'tag', k));
      item.appendChild(el('div', null, v)); head.appendChild(item); });
  } else head.appendChild(el('p', 'empty', t('noRoundYet')));
  box.appendChild(head);
  box.appendChild(el('h3', 'subhead', t('allQuotes')));
  const wrap = el('div', 'table-wrap'); const table = el('table');
  const hr = el('tr');
  [t('maker'), t('assetLabel'), t('ask'), t('bid'), t('reason')].forEach(h => hr.appendChild(el('th', null, h)));
  table.appendChild(hr);
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
    table.appendChild(tr);
  });
  wrap.appendChild(table); box.appendChild(wrap);
}
function updateObserver(){
  const d = V.observer || {}, e = obsEls;
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
    applyStrings();
    render(); };
  $('btn-leave').onclick = () => { send({type:'release'}); resetSeatState(); };
  $('btn-watch').onclick = () => send({type:'claim', seat:'observer', label:$('pname').value});
  // The drawer: the seat picker from the view switch, the seat's own controls
  // from the controls button, a card from the diagram (mouse, touch, Enter),
  // and Escape or the close button to put it away.
  $('btn-details').onclick = () => { detailsHidden = !detailsHidden;
    try { localStorage.setItem('qomm.detailsHidden', detailsHidden ? '1' : '0'); } catch (_) {}
    drawDetailsToggle(); drawNetworkGraph();
  };
  $('btn-view').onclick = () => { if (drawerPane === 'seats') closeDrawer(); else openPane('seats'); };
  $('btn-me').onclick = () => { const me = myPaneId(); if (!me) return; if (drawerPane === me) closeDrawer(); else openPane(me); };
  $('drawer-close').onclick = closeDrawer;
  $('hint-close').onclick = () => { hintClosed = true; markGuided(); drawHint(); };
  $('engine-more').onclick = () => {
    const full = $('engine-full'), more = $('engine-more');
    const open = !full.classList.toggle('hide');
    more.setAttribute('aria-expanded', open ? 'true' : 'false');
    more.textContent = t(open ? 'less' : 'more');
  };
  window.addEventListener('keydown', (e) => { if (e.key === 'Escape' && drawerPane){ e.preventDefault(); closeDrawer(); } });
  $('chat-input').addEventListener('keydown', e => { if (e.key === 'Enter'){ e.preventDefault(); handleChatInput(); } });
  $('chat-send').addEventListener('click', handleChatInput);
  let resizeTimer = null;
  const redrawIfWidthChanged = () => { clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      const graph = $('network-graph');
      // Seat or no seat: the diagram is on screen from the first view.
      if (V && graph && graph.clientWidth !== lastGraphWidth) drawNetworkGraph();
    }, 120); };
  window.addEventListener('resize', redrawIfWidthChanged);
  // The diagram is laid out for the width it had when the first view arrived;
  // if the box changes width afterwards (stylesheet, fonts, orientation), it is
  // laid out again.  Height changes are the diagram's own and do not redraw.
  if (typeof ResizeObserver !== 'undefined') new ResizeObserver(redrawIfWidthChanged).observe($('network-graph'));
  applyStrings();
  connect();
}
if (typeof module !== 'undefined' && module.exports){
  module.exports = {interpretTaker, interpretMaker, makerCashRequirement, translateRefusal, orthoPath,
    formatDuration, setLang: (l) => { lang = l; }, S};
}

// Assurance metadata is public; no provisional price or identity is broadcast.
function drawAssurance(){
  const a = V?.config?.assurance;
  const control = $('assurance-mode');
  if (!control) return;
  control.parentElement.hidden = !a;
  if (!a) return;
  const ja = lang === 'ja';
  $('assurance-label').textContent = ja ? '検証方式' : 'Assurance';
  control.options[0].textContent = ja ? '共同証明' : 'Joint proof';
  control.options[1].textContent = ja ? '暫定方式' : 'Optimistic';
  control.options[1].disabled = !a.optimistic_available;
  control.value = a.selection?.mode || 'joint_proof';
  control.disabled = !!P?.active || !a.selectable;
  const claim = P?.optimistic, status = claim?.status?.status;
  const box = $('assurance-status'), button = $('challenge-claim');
  box.hidden = !claim;
  if (!claim) return;
  const remaining = Math.max(0, Math.ceil(claim.challenge_deadline - Date.now()/1000));
  const names = ja
    ? {pending:'暫定結果を受理・未決済', challenged:'challenge中・元の証明を検証', proven:'証明を確認・確定期限待ち', finalized:P?.active ? '検証確定・DvP決済中' : P?.status === 'done' ? '台帳への決済確定を確認' : '検証確定・決済結果を要確認', rejected:'暫定結果を棄却・決済不可'}
    : {pending:'Provisional · unsettled', challenged:'Challenged · verifying proof', proven:'Proven · waiting for deadline', finalized:P?.active ? 'Assurance finalized · settling DvP' : P?.status === 'done' ? 'Settlement confirmed on ledger' : 'Assurance finalized · check settlement', rejected:'Rejected · settlement forbidden'};
  $('assurance-message').textContent = (names[status] || status || '') + (['pending','proven'].includes(status) ? (ja ? '（残り約' + remaining + '秒）' : ' (about '+remaining+'s)') : '');
  if (P?.provisional && P.active) $('assurance-message').textContent += (ja ? ' · 暫定価格 ' : ' · Provisional price ') + showPrice(V.assets,P.provisional.asset,P.provisional.price) + ' × ' + P.provisional.quantity;
  button.hidden = !a.challenge_available || status !== 'pending' || !P?.active || remaining === 0;
  button.textContent = ja ? 'challengeを送信' : 'Submit challenge';
  button.disabled = false;
}
$('assurance-mode').addEventListener('change', e => {
  send({type:'config', values:{assurance_mode:e.target.value}});
  e.target.disabled = true;
});
$('challenge-claim').addEventListener('click', e => {
  const claim = P?.optimistic;
  if (claim) { e.target.disabled = true; send({type:'challenge', claim_id:claim.claim_id}); }
});
