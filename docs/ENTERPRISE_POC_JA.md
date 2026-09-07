# QOMM 企業向けPoC導入ガイド

## このPoCで確認すること

QOMMは、Takerの問い合わせとMakerの価格規則・在庫を、Makerや単独の運営者へ
開示せずに照合する市場である。PoCでは、画面が動くことだけでなく、次を確認する。

- MakerがTakerの問い合わせを受け取らない。
- Takerは自分の結果だけを受け取る。
- Makerは自社の価格規則と最大在庫を事前登録する。
- 結果確認後の追加署名なしで、事前確保した資産だけが決済される。
- 不成立、期限切れ、MPC停止時には資産が動かない。
- ノードが扱うのは秘密分散された断片で、平文の注文や価格規則ではない。
- DeFMIを使う段階では、正本台帳の読戻しまで完了して初めて決済済みになる。

このリポジトリは研究実装であり、監査済み取引所、認可済み市場、または商用SLAを
提供するものではない。PoCでは実顧客情報、実資産、実運用鍵を使わない。

## 推奨する担当者

| 担当 | PoCでの役割 |
|---|---|
| 業務責任者 | 対象商品、価格条件、成立条件、例外処理を決める |
| Maker担当 | 価格規則、在庫上限、事前予約の妥当性を確認する |
| Taker担当 | 問い合わせ、指値、数量、期限と表示結果を確認する |
| MPC運用担当 | ノード分離、鍵、停止・再開、ログの秘密境界を確認する |
| 決済担当 | zkPIとDeFMIの予約・DvP・受領証を確認する |
| セキュリティ担当 | 通信、ログ、権限、バックアップ、攻撃試験を確認する |

一人が複数役を兼ねてもよいが、最終評価ではMaker、Taker、MPC運用者、決済運用者を
別担当にし、それぞれが実際に見える情報を記録する。

## 必要な環境

- インターネットへ公開しないLinux環境。
- Gitと、`Cargo.lock`を変更せず利用できるRust toolchain。リポジトリに
  `rust-toolchain.toml` はないため、PoC開始時の `rustc --version` を記録し、承認版を揃える。
  統合Docker buildが現在使う参照版はRust 1.97.1である。
- 画面確認用の現行ブラウザ。
- 実MPC確認では、別途構築した公式MP-SPDZ checkout。
- 実決済確認では、DeFMIガイドに記載したAvalancheGoと
  avalanche-network-runner。

取得した版、`rustc --version`、依存lockの要約値、外部実行ファイルの版と要約値を、
PoC開始前に記録する。最新版へ自動更新するスクリプトは使わない。

## 1. ソースと基準試験を固定する

```sh
git clone https://github.com/zkFMI/qomm.git
cd qomm
git checkout <社内で承認したcommit>
git rev-parse HEAD

cd rust
cargo test -j 4 --locked --workspace
cd ..
```

`Cargo.lock`が書き換わった場合は、そのまま進めず依存差分を確認する。試験の成功は
暗号運用や法的有効性を保証しないが、選んだ版が自己矛盾なく動くための最低条件になる。

## 2. 一台で役割と情報境界を確認する

次は説明用の `sim` エンジンである。秘密分散と役割別表示は動くが、価格比較は
MP-SPDZで実行せず、決済先も説明用のメモリ内台帳である。

```sh
cargo run -j 4 --release --manifest-path rust/Cargo.toml \
  -p qomm-harness --bin serve_demo -- \
  --host 127.0.0.1 --port 8800 --no-auto-rounds --seed 20260904
```

表示されたURLを開き、少なくとも次の席を別ブラウザプロファイルまたは別端末で開く。

```text
http://127.0.0.1:8800/?seat=taker&label=Taker
http://127.0.0.1:8800/?seat=maker:0&label=Maker-0
http://127.0.0.1:8800/?seat=node:0&label=MPC-0
```

Maker席とMPC席の番号は0から始まる。Taker席のIDには番号を付けない。

`observer`席は説明会用であり、実運用には存在しない全情報表示である。秘密性の
受入判定には使わない。

### 操作シナリオ

1. Makerが最大在庫、対象資産、価格規則を登録する。
2. Takerが数量と指値を含む問い合わせを作り、事前予約を確認して送る。
3. Maker画面に問い合わせ内容が表示されないことを確認する。
4. MPC席に自ノードの断片だけが表示されることを確認する。
5. 成立時にTakerと勝者Makerの残高だけが変わることを確認する。
6. 指値外、不成立、ノード停止を試し、残高と予約が戻ることを確認する。

チャット入力は固定規則で操作へ変換する補助UIであり、生成AIによる裁量的な取引判断ではない。

## 3. 実MP-SPDZで照合する

公式MP-SPDZをPoC用Linux上で構築し、checkoutを明示する。

```sh
export MP_SPDZ_ROOT=/absolute/path/to/MP-SPDZ

cargo run -j 4 --release --manifest-path rust/Cargo.toml \
  -p qomm-harness --bin serve_demo -- \
  --engine mpc --nodes 7 --threshold 2 \
  --mp-spdz-root "$MP_SPDZ_ROOT" \
  --host 127.0.0.1 --port 8800 --no-auto-rounds --seed 20260904
```

画面のエンジン表示が「MP-SPDZ実行（価格照合）」であり、起動ログに`engine mpc`と
出ること、平文参照計算とMPC結果が一致することを確認する。`--threshold T`は
MP-SPDZの`-T`、すなわち許容する不正party数であり、署名の`k-of-n`ではない。
実行には`n >= 2T + 1`、不正値の特定・訂正には`n >= 4T + 1`が必要になる。
7ノード・T=2では、`n >= 4T + 1`を必要とする任意の
「不正ノードを特定して訂正する」デモ条件を満たさない。通常のMPC実行と、任意の
不正訂正実験を混同しない。

この段階も一台のMP-SPDZ checkoutで複数partyを動かす評価であり、独立企業が運営する
7組織WANを証明しない。

## 4. DeFMI決済まで確認する

追加署名なしの実決済経路は、DeFMIリポジトリの
[`docs/ENTERPRISE_POC_JA.md`](https://github.com/zkFMI/defmi/blob/main/docs/ENTERPRISE_POC_JA.md)
に従って行う。最終段階では次を別プロセスまたは別コンテナにする。

- Maker法人参加モジュール。
- Taker法人参加モジュール。
- MPCノード7台。
- QOMM Gateway。
- DeFMI検証者5台。
- AvalancheGo。

合格条件は、署名済み問い合わせを送った後にMakerまたはTakerへ再署名を求めず、
資金legと資産legが同じ正本状態遷移で成立し、全検証者の状態根が一致することである。

## 5. 必須の失敗試験

- Maker在庫不足、Taker資金不足、法人合計枠超過。
- 同じ問い合わせ、予約、zkPIの再送。
- 指値外、期限切れ、適格Makerなし。
- MPCノード1台停止、処理途中停止、全ノード停止後の再開。
- Gateway再起動と、受付済み要求の耐久キュー復元。
- DeFMI検証者停止と復旧。
- 古い状態根、不足署名、壊れた証明。
- 二つの同時問い合わせが同じMaker在庫または法人枠を使う競合。

拒否された要求について、資産、予約、受付順、監査記録のどれが変化したかを確認する。
「エラーになった」だけでは合格にしない。

## 6. 保存する証拠

- Git commit、Cargo.lockのSHA-256、Rust版。
- MP-SPDZ commitと実行バイナリのSHA-256。
- ノード数、しきい値、Maker数、対象資産、乱数seed。
- 各役割の画面と、その役割へ届いたAPI応答。
- 成立、不成立、停止、再送、競合の実行ログ。
- zkPI指図のfingerprint、DeFMI取引ID、前後状態根、受領証。
- 前後の利用可能額、予約額、合計額の保存則。
- ログと永続ファイルに平文問い合わせ・価格規則・秘密鍵がないことの検査結果。

秘密値そのものを証拠フォルダへコピーしない。要約値、公開鍵、匿名化した役割名だけを残す。

## 7. PoC合格条件

- Makerは照合前後を通じてTakerの不成立問い合わせを受け取らない。
- Taker以外へ正確な個別価格を配信しない。
- 事前予約を超える約定と、結果確認後の任意拒否ができない。
- 同時要求でも資金・在庫・保証枠が負にならない。
- ノード停止時に別要求が密かに追い越さず、再開後も一回だけ処理される。
- DeFMI段階では正本読戻し前に決済済み表示をしない。
- 失敗試験が正本を変えず、監査可能な理由で拒否される。

## 本番移行前に別途必要なもの

- 契約、認可、最良執行、記録保存、紛争処理の法務設計。
- 実KYB発行者と法人グループ単位の枠管理。
- 独立主体が運営するMPCノードと鍵生成式。
- HSM、TLS/mTLS、ネットワーク分離、監視、バックアップ、災害復旧。
- 外部セキュリティ監査、暗号レビュー、負荷・WAN・侵入試験。
- 市場停止、価格異常、oracle停止、参加者破綻時の運用手順。

PoC合格は、これらの本番要件を満たしたことを意味しない。

---

## 8. 技術構成を先に理解する

### 8.1 一件の問い合わせが通る経路

QOMMは、Web画面から秘密計算を直接呼ぶ単一サーバではない。企業PoCでは次の境界を
別プロセス、可能なら別コンテナまたは別VMとして扱う。

```text
Takerの業務システム
  -> Taker法人参加モジュール
     - DeKYX資格の提示
     - 資金・証券・保証枠の事前予約
     - RFQ/RFM/RFSの正規化
     - 7個の秘密share作成
  -> 固定時間枠のrelay / QOMM gateway
  -> MPC node 0..6
     - 各nodeは一つのshareだけ保持
     - Makerの価格規則・在庫shareを常駐保持
     - MP-SPDZ malicious Shamirで共同評価
  -> proof party 0..6
     - 入力範囲、価格上限、DvP条件の共同証明
     - FROSTによるthreshold承認
  -> zkPI
     - 指図、期限、予約、nullifier、domainを固定
  -> DeFMI
     - 予約済みの資金・資産を追加署名なしで原子的決済
  -> Maker/Taker法人参加モジュール
     - DeFMI正本を読み戻して在庫・資金表示を更新
```

どの一台にも、Takerの問い合わせ、全Makerの価格規則、全在庫、復元に必要なshareを
同時に置かないことが設計の中心である。

### 8.2 使用する主要技術

| 技術 | 実装上の用途 | 守るもの | 守らないもの |
|---|---|---|---|
| 加法的秘密分散 | Taker入力を7断片へ分ける | 単独nodeからの入力復元 | 閾値以上のnode共謀 |
| MP-SPDZ malicious Shamir | 価格規則、在庫、比較、勝者選択 | 計算中の値と不正share | 全node停止、外部入力の真実性 |
| 承認済みDSL | Maker価格規則を有限の回路へ変換 | 任意コード実行、隠れた外部通信 | 経済的に悪い価格設定 |
| Pedersen commitment | 値を隠して後から変更できなくする | 価格、数量、状態とのbinding | opening漏えい後の秘密性 |
| Bulletproof系範囲証明 | 値が許可範囲内と示す | 負値や範囲外値 | 入力の法的正当性 |
| FROST/Ristretto255 | 7 proof partyのうち設定数で署名 | 一台の署名鍵侵害 | threshold以上の共謀 |
| X25519 winner envelope | 勝者向け情報を選択的に開示 | 敗者への約定情報漏えい | endpoint metadata |
| DeKYX | 法人資格とscope別nullifier | 実名を見せない参加資格 | issuerの誤認定 |
| zkPI | MPC結果と予約を決済指図へ束縛 | 結果差替え、二重使用 | DeFMI自体の可用性 |
| DeFMI/Avalanche L1 | 取引順序と資産移転の正本 | 二重決済、片脚決済 | 外部法域での法的最終性 |
| 分散DP | 市場統計へ校正noiseを追加 | 一法人の寄与推定 | 個別quoteの漏えい全般 |

### 8.3 RustとMP-SPDZの境界

QOMMの製品コード、実験、ネットワークサービス、VMはRustで実装されている。秘密計算の
実行器にはMP-SPDZを利用し、次の二つの形がある。

1. RustからMP-SPDZのC++ライブラリ境界を呼ぶ組込み経路。
2. `malicious-shamir-party.x`をnodeごとの外部processとして起動する常駐/WAN経路。

MP-SPDZの公式`compile.py`は、承認済み計算programをMP-SPDZ bytecodeへ変換するために
残る。これはQOMMの実験を別言語で実装しているという意味ではない。企業PoCでは、
`compile.py`、MP-SPDZ commit、QOMM patchの要約を記録する。

### 8.4 7 nodeとthresholdの意味

一台デモの既定値は9 MPC node、`T=2`で、`n >= 4T + 1`を満たす設計点である。
一方、Docker統合デモと常駐MPC nodeは7 node、`T=2`に固定されており、不正値の
特定・訂正までは主張しない。いずれのmalicious Shamir構成でも、
秘密復元・不正許容・能動的訂正の条件を一つの「5-of-7」などの言葉へ丸めない。

- MPCの秘密分散threshold。
- FROST署名の最小参加数。
- DeFMI委員会の承認数。
- Avalanche validatorのconsensus条件。
- proof partyの応答条件。

これらは別設定であり、同じ値とは限らない。PoC構成図に各値を別々に記載する。

## 9. 推奨ハードウェア

以下は性能保証値ではなく、PoC開始時の資源不足を避ける初期値である。最終的な構成は
対象Maker数、価格規則の幅、slot頻度、同時Taker数、WAN遅延、証明方式を固定して実測する。

### 9.1 説明用の一台構成

| 項目 | 初期推奨値 |
|---|---:|
| CPU | 8 vCPU以上 |
| メモリ | 16 GiB以上 |
| ディスク | 空き50 GiB以上のSSD |
| OS | 64-bit Linux |
| 用途 | `sim` UI、少数Maker、機能説明 |

一台構成は、秘密性や独立運営を証明しない。同じprocessまたは同じ管理者が全shareへ
アクセスできるため、暗号protocolの配線確認に限る。

### 9.2 一台で実MP-SPDZを動かす構成

| 項目 | 初期推奨値 |
|---|---:|
| CPU | 16–32 vCPU |
| メモリ | 32–64 GiB |
| ディスク | 100 GiB以上のNVMe/SSD |
| network | loopbackまたは同一host bridge |
| 用途 | 7 party process、回路・証明・機能測定 |

同一hostの7 processは、回路の正しさと計算資源を測る。独立7組織、WAN、障害分離、
管理者共謀を再現しない。

### 9.3 7 node WAN PoC

各nodeを別VMまたは別物理hostへ置く開始値である。

| node種別 | 台数 | CPU/台 | メモリ/台 | disk/台 | network |
|---|---:|---:|---:|---:|---|
| MPC node | 7 | 8–16 vCPU | 16–32 GiB | 100 GiB SSD | 1 Gbps以上を開始値 |
| proof party | 7 | 4–8 vCPU | 8–16 GiB | 50 GiB SSD | MPC nodeと低遅延 |
| gateway | 2 | 4–8 vCPU | 8–16 GiB | 50 GiB | 外部/APIと内部を分離 |
| relay | 2以上 | 2–4 vCPU | 4–8 GiB | 20 GiB | 固定slot trafficを処理 |
| Maker参加module | Maker法人ごと | 2–4 vCPU | 4–8 GiB | 50 GiB | 企業内HSM/DBへ接続 |
| Taker参加module | Taker法人ごと | 2–4 vCPU | 4–8 GiB | 50 GiB | 企業内OMS/TMSへ接続 |
| 監視・証拠収集 | 1–2 | 4–8 vCPU | 16 GiB | 200 GiB | 秘密を集約しない |

MPC nodeとproof partyを同じhostに置くと運用は簡単だが、同じ管理者・kernel・disk侵害の
共通障害点になる。PoC報告では「論理分離」と「運営主体分離」を別欄にする。

### 9.4 CPU選定

- GPUはQOMM MPCの前提ではない。
- 高いsingle-core性能だけでなく、7 process、証明、network処理を並行できるcore数を見る。
- 同一世代CPUに揃えると比較しやすいが、実運営候補の異機種も別試験する。
- CPU frequency scalingとthermal throttlingを記録する。
- virtual CPUのovercommit率をcloud/virtualization管理者から取得する。
- AES-NI等の有無だけを暗号性能の代表値にしない。実回路を測る。

### 9.5 メモリ選定

次を同時に計上する。

- MP-SPDZ party process。
- preprocessing/通信buffer。
- 常駐Maker state。
- Rust node serviceとSQLite page cache。
- proof generationとFROST state。
- build時のCargo/rustc。運用hostではbuildしない。

最大RSSの2倍を初期余裕とし、swapが発生したrunは性能合格に使わない。

### 9.6 disk選定

保存対象はbinaryだけではない。

- node別SQLite。
- encrypted resident state。
- FROST DKGとnonce消費状態。
- TLS証明書とrotation履歴。
- slot receipt、受付ticket、proof receipt。
- audit logとmetric。
- MP-SPDZ program、schedule、必要なpreprocessing。
- crash dumpを有効にする場合の最大容量。

PoCではNVMeで開始し、IOPS、fsync時間、queue深さを測る。disk full時は古い監査logを
自動削除して取引継続するのではなく、新規受付停止または安全な縮退へ移る。

## 10. ネットワーク設計

### 10.1 通信面を分ける

| 面 | 接続主体 | 内容 | 公開範囲 |
|---|---|---|---|
| participant API | Maker/Taker module ↔ gateway | 予約、要求、結果 | 参加企業閉域 |
| fixed-frame relay | participant ↔ relay/node | 495-byte slot frame | 専用閉域 |
| node control | coordinator ↔ node | 4096-byte制御frame | 運営内部 |
| MPC data | MPC party間 | MP-SPDZ share通信 | node間限定 |
| proof API | coordinator ↔ proof party | proof/FROST job | node間限定 |
| DeFMI handoff | gateway/proof ↔ DeFMI | zkPI、予約、receipt | settlement network |
| observability | service →監視 | metrics、構造化log | 監視領域 |
| administration | 管理端末 → service | rollout、鍵更新 | bastion経由 |

一つのingressやservice accountですべてを処理しない。特にmetricsと管理APIをparticipant
networkへ公開しない。

### 10.2 WAN遅延を決める

想定地域間のRTTを、最低、代表、悪化の三段階で測る。

```text
RTT matrix:
          node0 node1 node2 ... node6
node0       0    12    35       180 ms
node1      12     0    28       170 ms
...
```

平均RTT一個ではなく、全pairのp50/p95/p99、packet loss、jitterを保存する。MPCは多段通信を
行うため、最遅pairとround数が遅延へ効く。

### 10.3 firewall方針

- participantはMPC party portへ直接接続できない。
- MPC nodeは必要なpeer IP/portだけへ接続する。
- proof partyはMPC secret inputを受けない。
- DeFMI validatorの管理portをQOMM gatewayへ開けない。
- package downloadはbuild networkだけ許可する。
- runtime networkからGitHubやcrate registryへの外向き通信を不要にする。
- DNS失敗時にも既知peerへ接続できるか、名前解決を明示管理する。

これは本番相当配備の方針である。現在のdemo compose networkは `internal: false` で、containerの
外向き通信をcomposeだけでは遮断しない。demoをこの条件の証拠に使わず、host firewall、egress proxy、
Kubernetes NetworkPolicy等で遮断し、実際の接続試験を残す。

### 10.4 時刻同期

slot、ticket、期限、FROST job、zkPI expiryに時刻を使う。

- 全nodeは認証済み時刻源を使う。
- 許容clock skewを設定ファイルへ明示する。
- skew超過nodeは新規slotへの参加を停止する。
- wall clockと単調時刻を使い分ける。
- NTP補正で時刻が後退した場合のslot重複を試験する。

## 11. Dockerデモを構築する

この章は、`qomm/demo-network/`、`qomm/qomm_demo/`、`dekyx/`、`deccp/`を
同じ親ディレクトリに含む統合デモ配布物を対象とする。QOMMの単独checkoutに
`demo-network/compose.yaml`が含まれない版では、この章を実行済みとして扱わない。
その場合でも第1章から第3章までのQOMM単体確認は実行できるが、DeFMI、法人参加
モジュール、分散MPCを含むDocker統合試験には、対応する統合デモ配布物が必要である。

### 11.1 起動されるservice

`demo-network/compose.yaml`は、次を一つのDocker network内で分けて起動する。

- `defmi-network`: 5 validatorとRust製DeFMI VM。
- `mpc-0`〜`mpc-6`: 一つのshareだけを扱うMP-SPDZ party。
- `maker-0`〜`maker-3`: Maker法人参加module。
- `taker`: Taker法人参加module。
- `mpc-operator-0`〜`mpc-operator-6`: node運営法人module。
- `gateway`: 7 MPC serviceを調整するRust service。
- `frontend`: 画面配信だけを行うRust service。

ホストへ公開するのは画面`8800`とDeFMI確認`9650`である。gatewayの`8801`は
Docker network内部だけへ公開し、frontendがWebSocketを中継する。MPC nodeや
法人moduleのportもホストへ公開しない。

### 11.2 build前確認

```sh
git rev-parse HEAD
docker version
docker compose version
mkdir -p poc-output
test -f demo-network/compose.yaml
test -d ../dekyx
test -d ../deccp
docker compose -f demo-network/compose.yaml config \
  > poc-output/compose-resolved.yaml
```

build contextは `qomm`、`dekyx`、`deccp` の三つを含む親directoryである。QOMMだけを別directoryへ
copyすると、Aethelが参照するDeKYX、DeFMI VMが参照するDeCCPを解決できない。

### 11.3 起動

```sh
docker compose -f demo-network/compose.yaml up --build -d
docker compose -f demo-network/compose.yaml ps
docker compose -f demo-network/compose.yaml logs --no-color \
  > poc-output/compose-startup.log
```

確認先:

```text
http://127.0.0.1:8800/
http://127.0.0.1:9650/manifest
```

画面が表示されたことだけで起動成功としない。`docker compose ps`で全serviceの状態を確認し、
DeFMI manifest、MPC node health、gateway healthを別々に読取る。

### 11.4 demoの信頼境界

同一Docker hostでは、host rootが全volume、container memory、network trafficへアクセスできる。
したがって、デモは次を確認するものとする。

- service境界とAPI配線。
- Maker/Taker/MPC operatorの画面と操作。
- 在庫・資金・予約・quote・request・matching・settlementの状態変化。
- node停止、queue、再送、同時枠競合。
- DeFMI正本への反映。

独立運営者に対する秘密性、host管理者からの保護、実WAN性能は確認しない。

現在のdemo image buildは、MP-SPDZを固定commitからcloneし、AvalancheGo 1.14.2と
avalanche-network-runner 1.8.3を固定SHA-256でdownloadする。§10.3の「runtimeでdownloadしない」と
矛盾しないよう、build時の外向き通信と取得物をSBOMへ記録し、完成imageだけを隔離環境へ移す。
このbuildは `CONFIG.mine` を作らず、MP-SPDZの7 party分の `setup-ssl.sh` 鍵を同じimage layerへ
含める。これは一台demo用であり、独立運営者の鍵分離を満たさない。本番相当配備では各運営者が
自分の鍵だけを生成・mountし、全party鍵入りimageを配布しない。

### 11.5 停止と証拠保全

```sh
docker compose -f demo-network/compose.yaml logs --no-color \
  > poc-output/compose-final.log
docker compose -f demo-network/compose.yaml ps --all \
  > poc-output/compose-final-ps.txt
docker compose -f demo-network/compose.yaml down
```

`down -v`は状態volumeを削除するため、証拠取得前に実行しない。PoC終了後に削除する場合は、
対象compose project名とvolume一覧を確認し、企業の廃棄手順へ従う。

## 12. MP-SPDZを準備する

### 12.1 版を固定する

公式MP-SPDZ checkoutと、利用する実行経路に必要なQOMM patchを管理する。基準commitは
`9d809599ea6ce627216a389ca7d984fbb75d0cb9`である。

```sh
cd /absolute/path/to/MP-SPDZ
test "$(git rev-parse HEAD)" = 9d809599ea6ce627216a389ca7d984fbb75d0cb9
git rev-parse HEAD
git status --short
sha256sum compile.py
sha256sum /absolute/path/to/qomm/rust/qomm-mpc/patches/*.patch
```

PoC証拠には次を含める。

- upstream repository URL。
- exact commit。
- 適用patch名とSHA-256。
- compiler、OpenSSL、GMP、Boost等の版。
- `CONFIG.mine`。
- `malicious-shamir-party.x`のSHA-256。

基本の外部party実行にはQOMM patchを適用しない。Rustへの組込み経路では
`expose-machine-to-embedder.patch`、不正shareの位置特定・訂正実験では
`locate-inconsistent-shares.patch`と`robust-atlas.patch`が対象になる。
`macos-arm64-clang20.patch`はLinux PoCの対象外である。利用するpatchだけを
`git apply --check`で確認し、適用順とdigestを証拠へ残す。

### 12.2 Linux build例

```sh
sudo apt-get update
sudo apt-get install -y \
  automake build-essential clang cmake git \
  libboost-dev libboost-iostreams-dev libboost-thread-dev \
  libboost-filesystem-dev libntl-dev libsodium-dev libssl-dev \
  libtool m4 texinfo yasm libgmp-dev libmpfr-dev

cd /absolute/path/to/MP-SPDZ
make -j 16 malicious-shamir-party.x
./Scripts/setup-ssl.sh 7
```

この基本経路では`-DINSECURE`も`-DGFP_MOD_SZ=4`も必須ではない。`-DINSECURE`は
MP-SPDZのfake-offline `-F`を使う研究測定だけに限定し、その成果を本番候補と混同しない。
`GFP_MOD_SZ`を指定する場合は、MP-SPDZ本体とRust組込み側が同じ`CONFIG`を読むことを
確認し、片側だけへflagを足さない。企業の暗号reviewを通していないbinaryを本番networkへ
持ち込まない。

RustからMP-SPDZを組み込む経路も試す場合は、対応patchを適用した同じcheckoutで
`libSPDZ.so`も構築する。

```sh
make -j 16 libSPDZ.so
sha256sum malicious-shamir-party.x libSPDZ.so
```

### 12.3 compile.pyの位置付け

`compile.py`はMP-SPDZ公式compilerであるため依存として認める。ただし次を守る。

- QOMMの価格・実験logicをRust以外へ実装しない。
- compile対象programを承認済みDSL出力から生成する。
- compile時のprogram source、schedule、bytecodeのdigestを保存する。
- runtime nodeが任意programをcompileしない。
- node設定のapproved program digestと一致したbytecodeだけを起動する。

### 12.4 nodeごとのTLS素材

一台で`setup-ssl.sh 7`を実行したdirectoryを全nodeへそのままcopyすると、全秘密鍵を一台が
知る。WAN PoCでは各nodeが自分の秘密鍵だけを生成・保持し、公開証明書と必要なlinkだけを
配布する。`ResidentMpcConfig`の検査は、一nodeのTLS manifestへ他node秘密鍵が入る構成を
拒否する。

## 13. QOMM Rust workspaceを構築する

```sh
git clone https://github.com/zkFMI/qomm.git
cd qomm
git checkout <承認したcommit>
git rev-parse HEAD
sha256sum rust/Cargo.lock
rustc --version --verbose

cd rust
MP_SPDZ_ROOT=/absolute/path/to/MP-SPDZ \
  cargo build --workspace --release --locked -j 16
cd ..
```

buildは運用nodeごとに行わず、隔離したCI/build hostで一度行う。生成binaryごとにSHA-256、
target triple、compiler版、feature、linkしたlibSPDZを記録する。

### 13.1 主なbinary

| binary | 用途 |
|---|---|
| `serve_demo` | 一台デモと画面 |
| `qomm-demo` | Docker統合デモのgateway |
| `qomm_mpc_node` | デモnetworkの一MPC node |
| `qomm_participant_node` | 法人参加module |
| `qomm_frontend` | UI配信 |
| `serve_node` | 固定frameを受ける常駐node |
| `qomm_node_party` | WANの一party実行 |
| `serve_proof_party` | ZK/FROST proof party |
| `prepare_wan_deployment` | WAN配備素材の準備 |
| `discover_wan_inventory` | node inventory収集 |
| `provision_frost_cluster` | 分散FROST DKG調整 |
| `wan_acceptance` | WAN受入検査 |
| `zkpi-verify` | zkPI独立検証 |

利用するbinaryだけを配布し、`qomm-harness`の研究用runner一式を運用nodeへ置かない。

## 14. Makerのセットアップ

### 14.1 Makerが準備するもの

- DeKYXで検証可能な法人資格。
- 対応可能なasset、side、数量帯、期間。
- 秘密の価格規則と係数。
- 初期在庫とrisk limit。
- DeFMI上の資産・資金note。
- 最大供給量を裏付ける事前予約。
- quote policy署名鍵、予約鍵、winner disclosure鍵、決済鍵。
- 緊急停止・鍵失効の連絡経路。

### 14.2 価格規則

価格規則は任意のplugin codeではなく、承認済みDSLへ制限する。例:

```text
quote = reference_price
      + side_spread
      + size_bucket_adjustment
      + inventory_bucket_adjustment
      + volatility_bucket_adjustment
```

秘密にできるのは係数、在庫、個別判定である。使える入力、演算、最大回路幅、出力範囲、
state updateは公開されたtemplateへ固定する。

### 14.3 Maker state

各Makerの状態を次へ束縛する。

```text
maker_id
policy_template_digest
secret_parameter_commitment
inventory_commitment
reserved_inventory_commitment
risk_limit_commitment
reference_market_epoch
state_sequence
valid_from / valid_until
active_flag
```

新しいquote奉仕期間を開始する前に、最大在庫をDeFMIでlockし、lock receiptをMPC stateへ
束縛する。問い合わせを見てから署名する手順を入れない。

### 14.4 更新

- 更新は次slotから有効にし、途中slotへ遡及しない。
- 新旧state digestとsequenceを記録する。
- inventory updateとDeFMI予約残を照合する。
- 更新中も固定trafficを止めない。
- Maker停止は価格を極端値にするのではなく、明示的eligibility flagで扱う。
- 古いstateを使った計算をproof/receiptで拒否する。

## 15. Takerのセットアップ

### 15.1 Takerが準備するもの

- DeKYX資格と法人/支配group単位のscope nullifier生成手段。
- OMS/TMSからRFQへ変換するadapter。
- 資金または売却資産のDeFMI事前予約。
- 一要求・一日・一法人groupの数量/回数上限。
- 7 node向けshare生成と送信。
- 結果復号鍵とsettlement追跡。

### 15.2 RFQ正規化

外部入力をそのまま秘密分散せず、次を検査する。

- assetが商品registryに存在する。
- sideが許可値。
- quantity、limit、期限が整数の固定単位で表現できる。
- 浮動小数を使わない。
- 時刻とslotが許容範囲。
- 予約receiptが同じasset、side、上限、法人へ束縛される。
- request ID、nonce、idempotency keyが一意。
- DeKYX presentation contextが同じrequestへ束縛される。

### 15.3 秘密分散

Taker moduleは各fieldを7 shareへ分け、nodeごとのpayloadを作る。すべてのshareをlogや
一つのDBへ保存しない。再送が必要な場合は、同じrequestに対してprotocolが要求する同じ
意味のframeを再現できるよう、encrypted outboxと一意性を管理する。

### 15.4 探り注文の制限

wallet単位ではなく、DeKYX発行者が署名した法人または支配group単位で次を持つ。

- slot当たり要求数。
- 時間窓当たり総数量。
- asset group別の数量。
- 不成立を含む問い合わせ回数。
- DP公開値へ与えたprivacy budget。
- 異常な取消・失敗比率。

この制限自体を公開すると活動量が漏れる場合は、MPC内で消費し、外部には許可/拒否と
監査可能な要約だけを返す。

## 16. 一件のRFQを操作する

### 16.1 事前条件

送信前に次を画面またはAPIで確認する。

```text
Taker qualification        valid
Taker reserve              active, sufficient, unexpired
Maker standing pools       at least the venue minimum
MPC nodes                  configured set healthy
proof parties              signing quorum available
approved program digest    identical on every node
reference market epoch     current
DeFMI network              accepting transactions
```

「node 7台のうち5台がhealthy」のような一つの総合表示だけでなく、MPC、proof、DeFMIの
各条件を別々に表示する。

### 16.2 要求作成

Taker画面またはOMS adapterで次を入力する。

- 商品。
- 買い/売り。
- 数量。
- 任意のlimit。
- RFQ/RFM/RFSの種別。
- 有効slotまたは期限。
- 決済条件。

画面には実名Maker候補や個別在庫を表示しない。送信前確認では、予約される最大資金・資産、
期限、取消不可になる時点を明示する。

### 16.3 受付

Taker moduleは、実要求の有無にかかわらず固定slot frameを送る。実要求については
受付ticketを受け取り、次を保存する。

```text
request_id
slot
frame_digest per node
ticket per node
reservation_id
qualification_context
submitted_at
```

一nodeのticket欠落を、別nodeのticketで補完しない。slot close条件どおりに処理する。

### 16.4 計算結果

利用者へ返す情報は次へ限定する。

- 成立/不成立。
- 成立時の正確な利用者価格。
- 有効期限。
- 決済追跡ID。
- 必要なら勝者との最小限の決済情報。

全Maker価格、二番手、在庫、拒否理由の詳細、内部比較traceを返さない。不成立時も
「価格が悪い」「在庫不足」「資格不一致」を細かく返すと探りに使われるため、公開error
taxonomyを事前設計する。

### 16.5 決済

成立結果からthreshold zkPIを作り、事前予約済みのlegsへ束縛する。MakerまたはTakerが
結果を見てから追加署名する手順は置かない。DeFMI確定後にだけ次を更新する。

- Maker/Takerの利用可能在庫・資金表示。
- 予約の消費または解放。
- QOMM requestの最終状態。
- 監査receipt。

MPC成功、proof成功、zkPI作成、DeFMI受理、DeFMI確定を別状態として保持する。

## 17. 複数RFQと順序

### 17.1 なぜ到着時刻だけで決めないか

gatewayが平文内容を見て到着順を選ぶと、特定要求を先にする、遅らせる、front-runする
余地が生じる。QOMMでは、固定slot内の受付ticketと後から確定する乱数を使い、内容に
依存しない順序を作る。

### 17.2 順序確定の記録

各slotについて次を保存する。

```text
slot_id
expected_participant_set_digest
ticket_set_root
late_entropy_commitment
late_entropy_reveal
ordered_request_ids_or_commitments
program_digest
state_root_before
state_root_after
```

`late_entropy`を受付完了前に知れる運営者がいると順序操作余地が残る。commit/revealまたは
threshold生成で、受付集合確定後まで値を固定・秘匿する。

### 17.3 同じMaker poolへの競合

複数RFQが同じ在庫を消費する場合、各要求を独立に「在庫十分」と判定してからまとめて
更新してはいけない。確定順にstateを更新し、後続要求は更新後残高を使う。

```text
state_0
  -- request A --> state_1
  -- request B --> state_2
  -- request C --> state_3
```

slot内を並列化する場合も、同じpool/法人枠へ触る要求は同じserializable partitionへ置く。

### 17.4 法人合計枠

同一法人が複数asset、複数wallet、複数request IDを使っても、合計予約が法人または
control group上限を超えないようにする。DeFMI/DeCCP側で原子的reservation sequenceを持ち、
QOMMは署名付きreceiptを参照する。

## 18. RFMとRFS

### 18.1 RFM

買値・売値を同じ秘密requestから評価する。二方向を別要求として送って活動量を二倍に
見せない。どちらを執行するか、または双方を情報として受けるだけかをrequest typeへ固定する。

### 18.2 RFS

同じ秘密subscriptionに対して時刻ごとのquote列を返す。

```text
q_i,t = P_i(x, s_i,t, m_t)
s_i,t+1 = U_i(s_i,t, fill_t)
```

必要な設計:

- subscription開始時のTaker事前予約。
- 各epochの参照市場root。
- Maker state sequence。
- quote expiry。
- fill時だけのstate update。
- subscription終了、timeout、取消規則。
- 実subscriptionがなくても固定epoch jobを作るdummy処理。

RFSは固定周期処理と相性がよいが、長時間の予約で資本を拘束する。予約費用、最大期間、
更新頻度を市場設計として決める。

## 19. FROST DKGとproof party

### 19.1 中央生成しない

7 shareを一台で作って配ると、その一台が全署名鍵を復元できる。PoCでも
`provision_frost_cluster`等の分散DKG経路を使い、各proof partyが自分の秘密packageだけを
保存する。

### 19.2 peer manifest

各partyについて次を固定する。

```text
party number
identity public key
exchange public key
network endpoint
deployment/session ID
approved circuit/program digests
storage key epoch
```

全partyが同じmanifest digestを確認してからDKGを進める。途中でpeerを差し替えない。

### 19.3 nonce

FROST signing nonceの再利用は秘密鍵漏えいにつながるため、予約済み、消費済み、完了済みを
永続化する。process crash後にmemoryだけを復旧して同じnonceを使わない。

### 19.4 署名対象

署名文へ最低限次を含める。

- protocol/domain version。
- QOMM deployment ID。
- Chain ID / DeFMI domain。
- request/slot/job ID。
- program digest。
- input/state commitments。
- output/settlement handoff digest。
- expiry。

異なる環境や用途の署名へ流用できないdomain separationを行う。

## 20. DeKYXと法人単位制御

### 20.1 生のKYC/KYBをQOMMへ入れない

QOMM nodeが必要とするのは、参加資格、商品資格、法人またはgroup単位の制限を適用できる
仮名参照である。登記簿、代表者名、住所、本人確認書類をnode DBへ保存しない。

### 20.2 issuer governance

PoCで定義する項目:

- 信頼するissuer IDと公開鍵。
- issuer namespace。
- accepted subject kind。
- 必須資格。
- status list更新頻度。
- 失効反映SLO。
- 鍵rotation grace。
- control group認定責任者。

### 20.3 privacy budget

DP統計を追加する段階では、公開queryごとのepsilonをwalletではなく法人/groupへ消費する。
budget ledger自体が活動量を漏らさないよう、外部には残予算の正確値を返さず、許可/拒否と
監査可能な証明を返す。

## 21. 設定管理

### 21.1 保存すべき設定digest

| 設定 | 変更時の影響 |
|---|---|
| Maker DSL template | MPC programとproof circuit |
| field bit width | range proof、wire、overflow条件 |
| Maker数上限 | 回路幅、性能、dummy entry |
| MPC node set/threshold | 秘密性、可用性 |
| proof party set/FROST threshold | zkPI承認 |
| slot duration | latency、dummy traffic量 |
| deadline/skew | 受付と再送 |
| DeKYX issuer registry | 参加資格 |
| DeFMI chain/domain | 決済先 |
| limit/guarantee policy | 予約可能量 |
| DP parameters | 公開精度とprivacy budget |

すべてにversion、valid-from、approver、digestを持たせる。nodeごとの設定が一致することを
startup時とslot開始時に確認する。

### 21.2 secrets

secrets managerまたはHSMへ置くもの:

- TLS秘密鍵。
- frame認証鍵。
- storage encryption key。
- Maker/Taker署名鍵。
- X25519開示鍵。
- FROST key package。
- DeFMI委員会/参加module鍵。

Git、Docker image、compose file、環境変数dump、command lineへ秘密値を入れない。

## 22. 常駐serviceの運用

### 22.1 起動順

1. DeFMI networkと正本readback。
2. DeKYX registry/status source。
3. participant modulesと予約adapter。
4. MPC node local storage、TLS、approved programs。
5. proof partiesとFROST group確認。
6. relay/gateway。
7. frontend。
8. synthetic dummy participant。
9. 最小のdummy slotとhealth receipt。

依存serviceがない状態でgatewayだけをhealthyにしない。

### 22.2 readinessとliveness

- liveness: processがloopを実行できる。
- readiness: 現在slotを安全に受けられる。

readinessにはpeer、program、state、time、disk、DeKYX status freshness、DeFMI接続を含める。
MPC計算不能なのにHTTP 200を返すhealth checkは使わない。

### 22.3 rolling update

slot途中にbinaryを混在させない。新versionを別node setまたは次epochへ用意し、全nodeが同じ
binary/program/config digestを確認してから切り替える。FROST keyを流用する場合は、
participant set、circuit width、domainが同じかを明示検査する。

## 23. 監視

### 23.1 実装を追加すべきmetrics

次は運用設計上の推奨名であり、現行serviceにPrometheus exporterや`/metrics`として
実装済みではない。現状取得できるのは各serviceの`/health` JSONと個別の実行成果物である。
PoCで監視adapterを追加した場合は、次の名前と秘密情報を含まないlabel規則を採用する。

```text
qomm_frames_total{node,result}
qomm_frames_bytes_total{node,dummy_or_real_not_exposed}
qomm_slot_close_duration_seconds
qomm_slot_missing_frames{node}
qomm_mpc_jobs_total{program,result}
qomm_mpc_duration_seconds{program}
qomm_mpc_rounds{program}
qomm_proof_duration_seconds{proof_type}
qomm_frost_jobs_total{result}
qomm_reservations_total{role,result}
qomm_reservation_conflicts_total{scope}
zkpi_total{result}
defmi_finality_seconds
qomm_outbox_depth{destination}
qomm_state_sequence{node_role}
```

`maker_id`、`taker_id`、asset、request ID、実/ダミーの区別をmetrics labelへ出さない。

### 23.2 最初のalert

- 一slotで予定frameが欠落。
- node間program digest不一致。
- node state sequence不一致。
- MPC result/proof binding不一致。
- FROST nonce再利用検出。
- reservation総量が容量を超える。
- DeFMI確定待ちが基準の3倍。
- outbox最古が5分超。
- disk 70/85/95%。
- clock skewが許容値超過。
- dummy traffic量が予定から外れる。

## 24. 障害注入

### 24.1 必須ケース

| 障害 | 注入点 | 期待動作 |
|---|---|---|
| MPC node 1台停止 | slot受付後 | 設定した可用条件内なら継続、そうでなければ全体abort |
| threshold超のnode停止 | 計算中 | 勝手な推定結果を出さず失敗 |
| proof party停止 | 証明中 | threshold未満ならzkPI未発行 |
| gateway停止 | Taker予約後 | participant outboxに残り、再開後一度だけ処理 |
| DeFMI停止 | zkPI作成後 | pending、正本照会後に再送 |
| Maker module停止 | standing pool有効中 | 既登録policy/予約の範囲で処理、期限後停止 |
| Taker module停止 | request送信後 | 結果を見て拒否できず、成立なら事前承認で決済 |
| stale reference | MPC開始前 | epoch検査で拒否 |
| corrupted share | MPC入力 | MAC/consistency検査で失敗 |
| duplicate frame | 同slot | 同一なら冪等、別内容なら衝突拒否 |
| late frame | slot close後 | 次slotへ勝手に移さず期限切れ |
| DB rollback | node再起動 | peer receipt/state root不一致でreadiness失敗 |
| FROST state loss | signing途中 | nonce再利用せずjobを安全に破棄・照会 |
| corporate cap race | 2 RFQ同時 | 合計上限以内だけ成功 |

### 24.2 情報漏えいを確認するケース

- 実requestなしと不成立requestで送信byte数が同じ。
- 実requestなしとありでslot timingが許容範囲内。
- Maker logにTaker入力がない。
- MPC node一台のdisk/memory dumpだけでは入力を復元できない。
- observer UIを本番構成から除外している。
- error messageでMakerごとの在庫・価格を推測できない。
- retry回数が実requestの有無を外部へ示さない。

## 25. 性能測定

### 25.1 事前固定する変数

- Maker数。
- active Maker比率。
- RFQ/RFM/RFS。
- side。
- policy templateとfield幅。
- MPC node数とthreshold。
- proof種類。
- slot時間。
- 同時Taker数。
- WAN RTT/loss/jitter。
- DeFMI validator数。
- CPU、memory、disk、container limit。

条件を変えながら一つのp95だけを比較しない。

### 25.2 最終利用者metric

```text
submit_to_ticket
ticket_to_slot_close
slot_close_to_mpc_result
mpc_result_to_proof
proof_to_zkpi
zkpi_to_defmi_accept
defmi_accept_to_final
submit_to_final
```

成功requestだけでなく、不成立、limit拒否、node停止、再試行も測る。

### 25.3 throughput

slot当たりrequest数とslot頻度から入口throughputを計算する。MPC回路がMakerを横方向に
並べられても、比較tree、proof、state update、決済が同じ割合で伸びるとは限らない。

### 25.4 資源

- nodeごとのCPU user/system時間。
- 最大RSS。
- network send/receive bytes。
- disk write/fsync。
- MP-SPDZ communication rounds。
- proof sizeと生成・検証時間。
- FROST message数。
- DeFMI transaction bytesとfinality。

## 26. セキュリティ確認

### 26.1 脅威表

| 脅威 | 主な対策 | 残る条件 |
|---|---|---|
| 運営者のfront-running | 注文share、内容非依存順序、commit/reveal | metadataやthreshold共謀 |
| Makerへの探り | 問い合わせ非配信、法人budget、DP | 正確価格を受ける正当利用者の反復 |
| Maker戦略漏えい | secret policy/state MPC | 多数の合法queryからの経済推定 |
| node改ざん | malicious-secure MPC、proof、receipt | protocol外のavailability攻撃 |
| selective abort | 固定epoch receipt、challenge、slashing設計 | 最終出力保証がない構成 |
| Sybil | DeKYX control group | issuerがgroupを見落とす場合 |
| 二重決済 | zkPI nullifier、DeFMI consumed state | 別domainへの誤binding |
| 枠超過 | DeFMI/DeCCP atomic reservation | 外部台帳adapterの不整合 |
| 鍵侵害 | HSM、threshold、rotation | threshold以上の侵害 |

### 26.2 query-oblivious accountability

challengeや失敗receiptが「この時刻に実requestがあった」と漏らさないよう、全slotでdummyを
含む同型jobとreceiptを作る。実requestのときだけ監査jobを起動しない。

## 27. トラブルシューティング

### MP-SPDZがbuildできない

OS、compiler、`CONFIG.mine`、依存library、upstream commit、patch適用結果を確認する。
別commitへ黙って切り替えない。

### `--engine mpc`なのに起動しない

`MP_SPDZ_ROOT`、`malicious-shamir-party.x`、libSPDZ、TLS素材、program digestを確認する。
simへ自動fallbackして成功扱いにしない。

### slotがcloseしない

予定participant集合、各node ticket、dummy sender、clock skew、late frame、HMAC拒否を確認する。
欠けたclientを集合から手動削除して同じslotを続けない。

### nodeごとに結果が違う

即時停止対象である。program、input receipt、state root、reference epoch、binary/config digestを
保全する。多数決で一つを採用しない。

### Maker在庫とDeFMIが違う

QOMM表示を手修正せず、最後に一致したstate sequence、reservation receipt、zkPI、DeFMI root、
再送履歴を照合する。DeFMI正本に基づくreconciliationを行う。

### requestが二回settleしたように見える

request ID、zkPI nullifier、DeFMI transaction ID、participant outbox/inboxを確認する。UI行が
二つでも正本移転が一回なら表示重複、正本が二回なら重大事故として新規受付を止める。

### FROST署名が作れない

peer manifest、DKG session、public package、ready node、nonce予約/消費、authorization digestを
確認する。shareを中央へ集めて代替署名しない。

## 28. 企業PoCの実施順

### Phase A: 配線と役割

1. sim UIを起動する。
2. Maker/Taker/MPC席を分ける。
3. 在庫、資金、quote、request、matchingを操作する。
4. observer情報が本番には存在しないことを確認する。

### Phase B: 一台の実MPC

1. MP-SPDZ版とbinaryを固定する。
2. 7 processで同じrequestを実行する。
3. 平文referenceとの一致を試験する。
4. node停止、share改ざん、再送を試す。

### Phase C: Docker統合

1. 法人module、7 MPC、gateway、frontend、DeFMIを分離する。
2. 事前予約から追加署名なし決済まで通す。
3. live acceptanceの停止・競合scenarioを実行する。

### Phase D: WAN

1. 7 nodeを別VM/region/運営担当へ配置する。
2. 分散TLS/FROST DKGを行う。
3. RTT matrixと秘密素材所在を監査する。
4. 代表負荷と障害を実行する。

### Phase E: 企業業務接続

1. OMS/TMSからTaker requestを作る。
2. Maker risk engineから承認済みpolicy/stateを登録する。
3. DeKYX issuerと失効を接続する。
4. 会計・在庫表示をDeFMI正本readbackへ接続する。
5. 業務継続と監査手順を実施する。

## 29. 最終成果物

1. 全service、運営主体、host、network zoneの配置図。
2. MPC、FROST、DeFMI、validatorの各threshold表。
3. source commit、lock、MP-SPDZ commit/patch、binary digest。
4. Maker policy template、state、予約の版管理表。
5. DeKYX issuerとcontrol group方針。
6. 一件のrequestからDeFMI確定までのtrace。
7. 全障害注入結果。
8. WAN RTT matrixと資源使用量。
9. privacy漏えい検査結果。
10. 枠超過、二重使用、再送の不変条件結果。
11. 未解決事項と本番化しない理由。
12. 次段階の責任者、期限、承認。

PoCを「秘密計算のデモ」に縮めず、利用者の事前予約、Makerのstanding pool、法人単位制御、
MPC、proof、zkPI、DeFMI正本、再起動回復を一つの業務経路として評価する。
