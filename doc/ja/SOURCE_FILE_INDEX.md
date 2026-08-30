# QOMM・DeFMI・zkPI ソースファイル索引

この付録は、[全ソースコード技術解説](SOURCE_CODE_GUIDE.md)から個々の実装へ移動するための索引である。第一者コードを、製品コード、実験コード、試験、比較実装に分けて列挙する。

## 1. 数え方

- 外部依存、生成物、測定JSON、論文原稿は数えない
- `rust/qomm-mpc/shim/qomm_spdz.cpp` と各シェルスクリプトも実行コードとして含める
- QOMM所有の製品・実験・編成コードはRustである。例外はMP-SPDZへ接続するC++ ABI、配備用シェル、比較対象のチェーンコード、画面資産だけである
- `rust/vendor/avalanche-rs-qomm`は出所とライセンスを固定した外部プロトコル境界なので、第一者コード数には含めない

以下の「試験」は、各ファイルが主に固定している契約を説明する。ファイル名だけでなく、何が壊れたときに失敗するかを示す。

## 2. `qomm-zk`: 共通暗号部品

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-zk/src/lib.rs` | 公開モジュールをまとめる |
| `rust/qomm-zk/src/pedersen.rs` | Ristretto255上のPedersenコミットメントと資産別生成元 |
| `rust/qomm-zk/src/sigma.rs` | 開示、同値、積、線形、0/1のσ証明とバッチ検証 |
| `rust/qomm-zk/src/range.rs` | Bulletproofsによる8/16/32/64ビット範囲証明 |
| `rust/qomm-zk/src/bitrange.rs` | 任意幅のビット分解型範囲・上下限証明 |
| `rust/qomm-zk/src/or_dleq.rs` | Chaum–PedersenをOR合成した一対多資格証明 |
| `rust/qomm-zk/src/oneofmany.rs` | Groth–Kohlweiss型の対数サイズ一対多証明 |
| `rust/qomm-zk/src/adaptor.rs` | PvP用のアダプター署名、完成、秘密抽出 |
| `rust/qomm-zk/src/shamir.rs` | Shamir分散、再構成、Berlekamp–Welchによる不正断片位置特定 |
| `rust/qomm-zk/examples/range_compare.rs` | Bulletproofsとビット分解方式の速度・大きさ比較例 |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-zk/tests/sigma.rs` | 正しいσ証明を受理し、文脈・値・証明改変を拒否する |
| `rust/qomm-zk/tests/bitrange.rs` | 任意幅、上下限、範囲外、構成改変を検査する |
| `rust/qomm-zk/tests/oneofmany.rs` | 正しい集合所属と非所属・証明改変を区別する |
| `rust/qomm-zk/tests/shamir.rs` | 再構成、欠損、不正断片の位置特定能力を検査する |

## 3. `qomm-zkpi`: 支払・受渡し指図

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-zkpi/src/lib.rs` | `Bounds`、`Instruction`、`Issuer`、`Venue`、FROST DKGと検証 |
| `rust/qomm-zkpi/src/typed.rs` | 予約・消費・解除・決済の操作種別、Maker/Taker役割、両予約、mandate、価格証明、状態根を旧zkPIへ結ぶ |
| `rust/qomm-zkpi/src/typed_wire.rs` | 型付き実行文脈の固定バイナリ形式と厳格な復号 |
| `rust/qomm-zkpi/src/handles.rs` | 本人種から会場別に結合困難なハンドルを導出 |
| `rust/qomm-zkpi/src/wire.rs` | 固定版付きバイナリ形式の符号化、厳格な復号、仕様出力 |
| `rust/qomm-zkpi/src/wire_vectors.rs` | 正常・異常な相互運用ベクトルを生成 |
| `rust/qomm-zkpi/src/bin/verify.rs` | 標準入力から指図を読む独立検証CLI |

### 試験・測定

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-zkpi/tests/instruction.rs` | 範囲、期限、領域分離、署名、ヌリファイア二重使用、DKG |
| `rust/qomm-zkpi/tests/wire.rs` | round-trip、途中切れ、末尾余り、未知版、不正点、ベクトル |
| `rust/qomm-zkpi/tests/typed.rs` | 役割、方向、予約、事前承認、価格証明、状態根、型付きFROST署名 |
| `rust/qomm-zkpi/tests/threshold_instruction.rs` | 共同範囲証明から作る支払指図と改変拒否 |
| `rust/qomm-zkpi/benches/wire.rs` | 指図の符号化・復号・検証時間とバイト数 |

## 4. `qomm-dsl`: 価格規則の制限言語

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-dsl/src/lib.rs` | コンパイラの公開入口 |
| `rust/qomm-dsl/src/rule.rs` | AST、宣言、役割、証明義務、全体コンパイル |
| `rust/qomm-dsl/src/parse.rs` | `.rule` の字句・構文解析と位置付きエラー |
| `rust/qomm-dsl/src/interval.rs` | 各式の整数区間、オーバーフロー、必要幅を静的計算 |
| `rust/qomm-dsl/src/emit.rs` | MPCソースとZK義務計画を同じASTから出力 |
| `rust/qomm-dsl/src/registry.rs` | 原本、渡された回路、回路形状の要約を登録後の差し替えから守る。初回登録時の原本と回路の意味的一致は検査しない |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-dsl/tests/language.rs` | 許可演算、禁止識別子、範囲、次数、未使用宣言、構文エラー |
| `rust/qomm-dsl/tests/registry.rs` | DSL原本と実行回路・形状の差し替えを拒否する |

## 5. `qomm-mpc`: MP-SPDZ回路と実行接続

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-mpc/src/program.rs` | RFQ/RFM/RFS、適格判定、比較木、在庫更新、入力検査を含む決定的回路生成。既定の公開検査、RFSのparty 0開示、複数要求のマスク共有もここで決まる |
| `rust/qomm-mpc/src/inputs.rs` | 利用者・MM・参照価格の断片ファイルと平文正解を生成 |
| `rust/qomm-mpc/src/persistence.rs` | MP-SPDZ永続ワイヤと結果を厳格に読み、法と形を検査 |
| `rust/qomm-mpc/src/lib.rs` | MP-SPDZ組込み実行、方式選択、計測結果のRust API |
| `rust/qomm-mpc/build.rs` | `MP_SPDZ_ROOT` がある場合だけC++接続層をビルド・リンク |
| `rust/qomm-mpc/shim/qomm_spdz.cpp` | MP-SPDZエンジンを呼び、通信チャンネル別ラウンド・バイト数を取得 |
| `rust/qomm-mpc/src/bin/gen.rs` | コマンド行から回路ソースと入力を生成 |
| `rust/qomm-mpc/src/bin/party.rs` | 一つのMP-SPDZ参加者として組込みエンジンへ入る |
| `rust/qomm-mpc/src/bin/rounds.rs` | 同じ回路を複数回実行し、エンジン内カウンタを出力 |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-mpc/tests/program_parity.rs` | 生成ソースと期待する命令列・構造が一致する |
| `rust/qomm-mpc/tests/all_files_parity.rs` | 全設定で入力ファイル、正解、回路の契約が一致する |
| `rust/qomm-mpc/tests/circuit_is_oblivious.rs` | 実問い合わせとダミーで回路形状・開示経路が変わらない |
| `rust/qomm-mpc/tests/reference_invariance.rs` | 参照値変更が許可された出力だけへ影響する |
| `rust/qomm-mpc/tests/fill_fold.rs` | マスク付きfillとキーの折畳み・復元が正しい |
| `rust/qomm-mpc/tests/dvp_handoff.rs` | 現金額・乱数を含むDvP永続wireの順序と、価格評価後から共同証明へ渡す境界を固定する |

## 6. `qomm-proofs`: 資格、価格、状態、共同証明

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-proofs/src/lib.rs` | 証明モジュールの公開入口 |
| `rust/qomm-proofs/src/kyb.rs` | 匿名法人資格、署名済みコホート、スコープヌリファイア、法人予算 |
| `rust/qomm-proofs/src/policy_audit.rs` | MM価格係数のコミットメント、範囲、稼働bit、VSS断片監査 |
| `rust/qomm-proofs/src/rule_audit.rs` | DSL ASTを一度評価し、同じ評価から証明手順を作る |
| `rust/qomm-proofs/src/quote_proof.rs` | 価格式、適格判定、番兵値、勝者、全候補最小性の証明 |
| `rust/qomm-proofs/src/state_audit.rs` | 在庫上限と前後状態を結ぶ状態遷移証明列 |
| `rust/qomm-proofs/src/liquidity.rs` | 正確な人数を出さず最低MM数以上を共同証明 |
| `rust/qomm-proofs/src/threshold_sigma.rs` | Shamir断片から一つの通常σ証明を共同生成 |
| `rust/qomm-proofs/src/threshold_gadgets.rs` | 共有線形演算、積証明、ノンス封印、部分応答監査 |
| `rust/qomm-proofs/src/threshold_range.rs` | 共有bitから範囲証明を共同生成 |
| `rust/qomm-proofs/src/price_limit.rs` | Takerの秘密限界価格と約定価格の大小関係を共同証明 |
| `rust/qomm-proofs/src/opening_envelope.rs` | 値と乱数の開示片を一回限り受取人へ暗号化し、規定数のノード片だけで復元させる |
| `rust/qomm-proofs/src/threshold_quote.rs` | 旧・部分ベンチ向けの共同価格証明。製品経路は`threshold_gadgets`、`threshold_range`、ノード別ハンドオフを使う |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-proofs/tests/kyb.rs` | 匿名所属、期限、文脈、コホート、法人単位上限 |
| `rust/qomm-proofs/tests/policy_audit.rs` | 係数範囲、active bit、断片改変、登録署名 |
| `rust/qomm-proofs/tests/policy_kyb_port.rs` | ポリシー登録とKYB主体の結合を再現する |
| `rust/qomm-proofs/tests/rule_audit.rs` | DSL評価と証明評価がずれず、手順改変を拒否する |
| `rust/qomm-proofs/tests/quote_proof.rs` | 正しい勝者を受理し、価格・候補・市場・方向改変を拒否する |
| `rust/qomm-proofs/tests/state_audit.rs` | 状態列、在庫上限、古い起点、分岐を検査する |
| `rust/qomm-proofs/tests/liquidity.rs` | 最低人数、秘密人数、断片不足、改変を検査する |
| `rust/qomm-proofs/tests/threshold_sigma.rs` | 共同開示証明、断片不足、誤応答を検査する |
| `rust/qomm-proofs/tests/threshold_range.rs` | 共有bit、任意幅、範囲外、部分応答を検査する |
| `rust/qomm-proofs/tests/distributed_product_rounds.rs` | 商品DvP用の積・残量証明を7プロセスの二段階応答で組み立てる |
| `rust/qomm-proofs/tests/price_limit.rs` | Taker買い・売りの限界価格、方向改変、文脈再利用を拒否する |
| `rust/qomm-proofs/tests/threshold_quote.rs` | 共同価格証明、回路断片、候補省略、法変換を検査する |
| `rust/qomm-proofs/tests/joint_nonce.rs` | ノンス寄与の再利用・すり替え・欠落を拒否する |
| `rust/qomm-proofs/tests/product_attribution.rs` | 不正な積の部分応答をノードへ帰属する |
| `rust/qomm-proofs/tests/slot_collision.rs` | 別スロットの証明・記録を再利用できない |
| `rust/qomm-proofs/tests/paper_artifact_node_boundary.rs` | 論文成果物が主張するノード秘密境界をコードで固定する |

## 7. `qomm-transport`: 固定通信、常駐ノード、鍵、実行制御

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-transport/src/lib.rs` | 通信・ノードモジュールの公開入口 |
| `rust/qomm-transport/src/wire.rs` | 303バイトフレーム、法`2^255-19`、加法分散、HMAC |
| `rust/qomm-transport/src/client.rs` | 毎スロット各ノードへ実またはダミーフレームを一通送る |
| `rust/qomm-transport/src/relay.rs` | スロット単位に受信、検査、混合、転送、ノード受信箱 |
| `rust/qomm-transport/src/order.rs` | 受付券、後決め乱数、バッチmanifest、省略証拠 |
| `rust/qomm-transport/src/roles.rs` | 入力者・計算ノードの断片、署名、値幅、不正断片監査 |
| `rust/qomm-transport/src/binding.rs` | Pedersen/VSSで入力断片を登録コミットメントへ結ぶ |
| `rust/qomm-transport/src/selective_disclosure.rs` | X25519とChaCha20-Poly1305による勝者限定固定長封筒 |
| `rust/qomm-transport/src/key_management.rs` | scrypt/AES-GCM鍵庫、更新、失効、公開manifest、相互TLS証明書、非リンク状態lock |
| `rust/qomm-transport/src/executor.rs` | 許可済みソース、バイナリ要約、open fd実行、引数・環境制限。現行のソース結合ラッパーは要約表示だけでMP-SPDZ計算は行わない |
| `rust/qomm-transport/src/node_service.rs` | 4,096バイト制御記録、相互TLS、検査済み秘密鍵、接続上限、SQLite、KYB、再送、スロット確定 |
| `rust/qomm-transport/src/resident_quote.rs` | 回路形状別キャッシュ、常駐MP-SPDZ見積もり、平文正解照合 |
| `rust/qomm-transport/src/mandate.rs` | Maker価格規則mandateとTaker自動決済mandateの正規化署名、復号、期限検査 |
| `rust/qomm-transport/src/pretrade_authority.rs` | 事前承認、匿名KYB、受付帳票、DeFMI予約受領証の秘密ファイル境界 |
| `rust/qomm-transport/src/resident_mpc.rs` | 常駐MPC実行とノード別秘密状態の接続 |
| `rust/qomm-transport/src/proof_party.rs` | ノード専用の共同証明、全段階を暗号化保存するFROST DKG、一回限り乱数・署名、安全設定結合、上限付き通信 |
| `rust/qomm-transport/src/proof_client.rs` | 証明ノードへ相互TLSで入出力上限付きJSON要求を送り、状態変更要求を自動再送しないクライアント |
| `rust/qomm-transport/src/frost_coordinator.rs` | 7証明ノードのFROST DKGを準備・ジャーナル化・冪等確定する共通調整器 |
| `rust/qomm-transport/src/frost_cluster.rs` | 7独立プロセスへ公開DKG・署名メッセージだけを中継する3-of-7調整役 |
| `rust/qomm-transport/src/dvp_issuer.rs` | 秘密分散された数量・価格・予約から共同DvP証明を作る |
| `rust/qomm-transport/src/dvp_wire.rs` | DvP共同証明のノード要求・二段階応答を固定長バイナリ形式へする |
| `rust/qomm-transport/src/limit_issuer.rs` | Taker限界価格の共同証明を作る |
| `rust/qomm-transport/src/limit_wire.rs` | 限界価格証明のノード要求・応答を厳格に符号化する |
| `rust/qomm-transport/src/quote_issuer.rs` | MPC出力から価格証明用のノード別作業と公開結果を組み立てる |
| `rust/qomm-transport/src/quote_wire.rs` | 価格証明のノード要求・応答を厳格に符号化する |
| `rust/qomm-transport/src/zkpi_issuer.rs` | 実MPC結果から共同範囲証明付きzkPIを作る |
| `rust/qomm-transport/src/zkpi_wire.rs` | 共同zkPI発行のノード要求・応答を厳格に符号化する |
| `rust/qomm-transport/src/proof_codec.rs` | 曲線点、scalar、範囲・積証明をノード間形式へ安全に変換する共通部品 |
| `rust/qomm-transport/src/external_kyb.rs` | 外部事業者の署名済みKYB assertion、法人/支配グループ分離、期限・取消し・安全なファイル入力を検査する |
| `rust/qomm-transport/src/external_signer.rs` | CSD署名を固定外部プロセスまたはHSM/KMS adapterへ委ね、上限・時間切れ・返却署名を検査する |
| `rust/qomm-transport/src/settlement_handoff.rs` | 受付順、共同証明、FROST公開鍵、型付きzkPIをDeFMIへ渡す永続形式 |
| `rust/qomm-transport/src/settlement_finalization.rs` | DeFMI受領証と実行文脈を検査し、最終型付き署名を作る |
| `rust/qomm-transport/src/kyb_lifecycle.rs` | 資格発行、期更新、失効根、会場キャッシュ、監査記録の永続サービス |
| `rust/qomm-transport/src/rtt.rs` | TCP接続時間の測定 |
| `rust/qomm-transport/src/ethereum_rpc.rs` | 比較用データ収集の最小Ethereum JSON-RPCクライアント |
| `rust/qomm-transport/src/bin/serve_node.rs` | 一つの常駐ノードを設定から起動 |
| `rust/qomm-transport/src/bin/serve_qomm.rs` | 常駐見積もりサービスを起動 |
| `rust/qomm-transport/src/bin/seven_node_cluster.rs` | 7ノードのTLS・SQLite・遅延・再起動・冪等性を一括実行 |
| `rust/qomm-transport/src/bin/qomm_node_party.rs` | 一つのMPC・証明参加者を独立OSプロセスとして起動 |
| `rust/qomm-transport/src/bin/serve_proof_party.rs` | 証明・FROST参加者を、秘密ファイル保護・通信上限・接続上限付き相互TLSサービスとして起動 |
| `rust/qomm-transport/src/bin/provision_frost_cluster.rs` | 実WANの7証明ノードへ一度限りのFROST DKGを行い、全段階から再開できる公開鍵要約と復旧記録を保存 |
| `rust/qomm-transport/src/bin/wan_acceptance.rs` | 7ホスト上の両サービスについて、一意性、非loopback、相互TLS、完全証明モード、FROST鍵、上限付き再起動、状態継続を検査 |
| `rust/qomm-transport/src/bin/wan_proxy.rs` | 指定遅延を入れるTCP中継 |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-transport/tests/frost_dkg_recovery.rs` | 参加者確認後、第1段階途中、第2段階途中、最終確定途中の各停止から同じFROST初期化を再開でき、安全設定変更を拒否する |
| `rust/qomm-transport/tests/wire.rs` | 固定長、分散再構成、HMAC、スロット・ノード改変 |
| `rust/qomm-transport/tests/fixed_order.rs` | 締切、重複法人、後決め乱数、順序再現、省略証拠 |
| `rust/qomm-transport/tests/roles_binding.rs` | 署名断片、VSS、値幅、不正ノード帰属 |
| `rust/qomm-transport/tests/selective_disclosure.rs` | 勝者復号、非勝者、文脈・見積もり・署名改変、鍵更新 |
| `rust/qomm-transport/tests/key_management.rs` | 0600、誤パスフレーズ、改変、原子的更新、更新、失効、TLS |
| `rust/qomm-transport/tests/executor.rs` | 未登録回路、要約不一致、symlink、引数注入、時間切れ |
| `rust/qomm-transport/tests/node_service.rs` | 役割、KYB、固定長、保存、再送、本文変更、再起動、スロット不足 |
| `rust/qomm-transport/tests/mandate.rs` | 正規化署名、期限、フィールド改変、末尾余りを拒否する |
| `rust/qomm-transport/tests/pretrade_authority.rs` | 事前承認・予約受領証・受付順・匿名KYBの結合を検査する |
| `rust/qomm-transport/tests/kyb_lifecycle.rs` | 発行、更新、失効、別会場、期限、再利用、監査記録 |
| `rust/qomm-transport/tests/zkpi_issuer.rs` | 実MPC由来の共同zkPI、範囲外、部分応答改変を検査する |
| `rust/qomm-transport/tests/dvp_issuer.rs` | 共同DvPの積・残量・役割選択・ノード応答改変を検査する |
| `rust/qomm-transport/tests/external_kyb.rs` | 外部署名、対象、期限、状態、取消し、法人/支配グループ、安全でない入力ファイルを拒否する |
| `rust/qomm-transport/tests/external_signer.rs` | 最大要求、時間切れ、過大応答、誤鍵・誤署名、書込み可能実行ファイルを拒否する |

## 8. `qomm-audit`: 受領記録とDP公開

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-audit/src/lib.rs` | 監査モジュールの公開入口 |
| `rust/qomm-audit/src/receipts.rs` | 固定スロット受領記録、故障証拠、状態列、保証金処理 |
| `rust/qomm-audit/src/distributed_dp.rs` | 離散Laplace、有限台delta、MPCソース、予算状態 |
| `rust/qomm-audit/src/publication.rs` | 統計文、予算鎖、前証明書、複数ノード署名 |
| `rust/qomm-audit/src/publication_ledger.rs` | 集計、分散雑音、予算消費、3-of-7公開、再実行防止を原子的に永続化 |
| `rust/qomm-audit/src/locate.rs` | 不正断片位置特定を共通Shamir実装から再公開 |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-audit/tests/receipts.rs` | 二重署名、省略、古い市場・状態、欠落、分岐、slash |
| `rust/qomm-audit/tests/distributed_dp.rs` | 分布、有限台delta、予算、MPCソース、公開証明書鎖 |
| `rust/qomm-audit/tests/publication_ledger.rs` | 予算・公開の原子性、ノードと正本の再実行拒否、出力改変拒否 |
| `rust/qomm-audit/tests/locate.rs` | 許容数までの不正断片位置を特定し、超過時は失敗する |

## 9. `qomm-defmi`: 複数資産の秘密決済

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-defmi/src/lib.rs` | DeFMIモジュールの公開入口と機能フラグ |
| `rust/qomm-defmi/src/assets.rs` | 資産別生成元、転送ごとの盲検化タグ、登録集合所属 |
| `rust/qomm-defmi/src/ledger.rs` | 口座コミットメント台帳、発行、prepare/commit/unwind移転 |
| `rust/qomm-defmi/src/settlement.rs` | zkPIに結ぶ口座型DvP、数量×価格、両脚原子更新 |
| `rust/qomm-defmi/src/asset_link.rs` | 秘密資産タグのコミットメントとDeFMI資産IDを結ぶ証明 |
| `rust/qomm-defmi/src/product.rs` | mandate、匿名KYB、予約、限界価格、型付きzkPI、DvPを製品決済へ結ぶ唯一の入口 |
| `rust/qomm-defmi/src/notes.rs` | 閲覧・支出鍵、ノート、リング支出、シリアル、二重使用防止 |
| `rust/qomm-defmi/src/note_settlement.rs` | ノート型の証券・現金DvP |
| `rust/qomm-defmi/src/note_chain.rs` | CSD署名発行、一回限りノート、予約lock、使用済みserial、受渡・返却請求権を専用Avalanche正本へ投影する |
| `rust/qomm-defmi/src/netting.rs` | BIS Model 1/2/3、参加者別差額、サイクル宣誓 |
| `rust/qomm-defmi/src/ccp.rs` | 署名債務、更改、清算参加者、ウォーターフォール、破綻分離 |
| `rust/qomm-defmi/src/credit.rs` | 担保ヘアカット、秘密信用枠、階層順序、再適用防止 |
| `rust/qomm-defmi/src/pvp.rs` | 二台帳間のアダプター署名型payment-versus-payment |
| `rust/qomm-defmi/src/chain.rs` | チェーン中立のメモリ状態、状態根、エスクロー遷移、拒否理由 |
| `rust/qomm-defmi/src/facility.rs` | SQLite永続資産・口座・保証主体・法人合算枠・予約・受付順・商品バッチ・受領記録 |
| `rust/qomm-defmi/src/avalanche.rs` | 全商品操作のAvalanche RPC、送信前検査、確定後投影、停止窓の冪等復旧 |
| `rust/qomm-defmi/src/reconcile.rs` | 原簿総数とのコミットメント照合と差異位置探索 |
| `rust/qomm-defmi/src/register.rs` | 厳格CSV原簿、署名、総数型・個別位置型の取込み |
| `rust/qomm-defmi/src/viewing.rs` | 範囲別閲覧鍵、閲覧許可、限定支出開示、期間更新 |
| `rust/qomm-defmi/src/vetting.rs` | 固定群の盲検化ハンドルと一対多審査証明 |
| `rust/qomm-defmi/src/bin/settle_wasm.rs` | 同じ決済検証器をnative/WASIで測る入口 |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-defmi/tests/atomic_transfer.rs` | prepare/commit/unwindと価値保存 |
| `rust/qomm-defmi/tests/dvp.rs` | zkPI、証券量、現金額、二脚原子性、二重使用 |
| `rust/qomm-defmi/tests/notes.rs` | ノート走査、リング支出、シリアル、釣銭、改変 |
| `rust/qomm-defmi/tests/note_settlement.rs` | ノート型DvPの両脚・リング・指図結合 |
| `rust/qomm-defmi/tests/note_chain.rs` | CSD発行、匿名予約、委任消費、受渡・返却請求権、後段ノート化、改変・再利用拒否 |
| `rust/qomm-defmi/tests/netting.rs` | Model 1/2/3、差額、閉鎖、残高不足、サイクル |
| `rust/qomm-defmi/tests/ccp.rs` | 債務署名、更改、宣誓、参加者別ウォーターフォール |
| `rust/qomm-defmi/tests/credit.rs` | 信用枠、担保価値、階層順序、解決ID再利用 |
| `rust/qomm-defmi/tests/pvp.rs` | 署名適応、秘密抽出、二脚完成、時間切れ解放 |
| `rust/qomm-defmi/tests/chain.rs` | 状態根、遷移、エスクロー、期限、ヌリファイア整理 |
| `rust/qomm-defmi/tests/facility.rs` | 資産・口座、k-of-n、旧状態、冪等性、rollback、受領記録鎖 |
| `rust/qomm-defmi/tests/avalanche.rs` | RPC形式、TLS境界、応答ID、合意待ち、前後状態根一致 |
| `rust/qomm-defmi/tests/reconcile.rs` | 合計一致、差異、改変、二分探索、開示予算 |
| `rust/qomm-defmi/tests/register.rs` | CSV形式、署名、総数・位置、重複・不正行 |
| `rust/qomm-defmi/tests/viewing.rs` | 範囲派生、許可、期限、支出開示、期間外到着 |
| `rust/qomm-defmi/tests/vetting.rs` | 固定群、所属・非所属、文脈、群改変 |

### ベンチマーク

| ファイル | 測るもの |
|---|---|
| `rust/qomm-defmi/benches/settle.rs` | 口座型DvPの証明・検証・決済とバイト数 |
| `rust/qomm-defmi/benches/note_dvp.rs` | 匿名集合サイズ別のノートDvP |
| `rust/qomm-defmi/benches/rings.rs` | リング証明と状態根の拡大 |
| `rust/qomm-defmi/benches/ccp.rs` | 更改・清算参加者処理 |
| `rust/qomm-defmi/benches/pvp.rs` | 二台帳PvPと露出時間 |
| `rust/qomm-defmi/benches/reconcile.rs` | 総数照合と差異位置探索 |
| `rust/qomm-defmi/benches/same_chain.rs` | 同一チェーン上の二DeFMIとアダプター処理 |
| `rust/qomm-defmi/benches/vetting.rs` | 固定群審査証明の生成・検証 |

## 10. `qomm-sim`: 市場、攻撃、DP効果

### 製品・実験コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-sim/src/lib.rs` | シミュレーションモジュールの公開入口 |
| `rust/qomm-sim/src/market.rs` | tick/lot市場、MM価格式、固定済みの決定的乱数 |
| `rust/qomm-sim/src/engine.rs` | 通常方式とQOMM、公開A/B/C、MM信念・約定の時間発展 |
| `rust/qomm-sim/src/disclosure.rs` | 非公開、しきい値、法人寄与制限付き離散DP、予算合成 |
| `rust/qomm-sim/src/attackers.rs` | 受動・能動探り・複数wallet・外部情報攻撃とAUC |
| `rust/qomm-sim/src/audit.rs` | 二世界DP監査、Clopper–Pearson、多重比較補正 |
| `rust/qomm-sim/src/queries.rs` | 価格・ブロック範囲統計、感度、料金、予算 |
| `rust/qomm-sim/src/tapes.rs` | UniswapX/Bybitデータの読込みと市場時間への写像 |
| `rust/qomm-sim/src/experiment.rs` | 比較arm、seed、集計、出力構造 |
| `rust/qomm-sim/src/lab.rs` | 共有設定、ρ・ε走査、候補実験 |
| `rust/qomm-sim/src/bin/attackdump.rs` | 攻撃器の決定的な中間値を出す |
| `rust/qomm-sim/src/bin/auditdump.rs` | DP監査の中間値を出す |
| `rust/qomm-sim/src/bin/enginedump.rs` | 市場エンジンの逐次状態を出す |
| `rust/qomm-sim/src/bin/experiment.rs` | 単一実験をコマンド行から実行 |
| `rust/qomm-sim/src/bin/marketdump.rs` | 価格・注文流の決定的な市場列を出す |
| `rust/qomm-sim/src/bin/middump.rs` | 中間価格系列を出す |
| `rust/qomm-sim/src/bin/rngdump.rs` | 固定済みの決定的乱数列を出す |
| `rust/qomm-sim/src/bin/tapedump.rs` | 実データ読込み結果を出す |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-sim/tests/deterministic_contract.rs` | 市場・約定・乱数が固定済み回帰契約と一致する |
| `rust/qomm-sim/tests/attack_contract.rs` | 攻撃スコアと集計が旧実装と一致する |
| `rust/qomm-sim/tests/disclosure_deterministic_contract.rs` | 公開A/B/Cと予算が旧実装と一致する |
| `rust/qomm-sim/tests/attackers_sampling.rs` | 訓練・評価分離と標本抽出が偏らない |
| `rust/qomm-sim/tests/audit.rs` | 二世界監査、区間、多重補正、検出力 |
| `rust/qomm-sim/tests/dp_adjacency_is_the_released_one.rs` | 数学上の隣接関係が実際の公開値へ適用される |
| `rust/qomm-sim/tests/block_range_queries.rs` | ブロック範囲の感度、課金、重複法人 |
| `rust/qomm-sim/tests/range_queries.rs` | 価格帯問い合わせの感度、予算、結果 |
| `rust/qomm-sim/tests/selective_disclosure.rs` | 未成立問い合わせと成立取引の公開境界 |
| `rust/qomm-sim/tests/tapes.rs` | 実データ形式、時刻、欠損、上限、再生 |
| `rust/qomm-sim/tests/lab.rs` | 走査設定、seed、arm比較、出力の一貫性 |

## 11. `qomm-demo` とブラウザ画面

### サーバー・実演コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-demo/src/lib.rs` | 実演モジュールの公開入口 |
| `rust/qomm-demo/src/main.rs` | 設定解析、sim/mpc選択、サーバー起動、MPC煙試験 |
| `rust/qomm-demo/src/model.rs` | 平文の価格式と結果型 |
| `rust/qomm-demo/src/protocol.rs` | 断片、積、復元、不正・離脱・不足の実演プロトコル |
| `rust/qomm-demo/src/mpc.rs` | 外部MP-SPDZ回路をコンパイル・実行し平文正解と照合 |
| `rust/qomm-demo/src/room.rs` | 席、役割別投影、ラウンド、在庫、約定、操作権限 |
| `rust/qomm-demo/src/bots.rs` | 空席の利用者・MM・ノードbot |
| `rust/qomm-demo/src/web.rs` | HTTP、RFC6455 WebSocket、接続別状態配信 |
| `qomm_demo/static/index.html` | ロビーと取引画面の静的構造 |
| `qomm_demo/static/demo.js` | 接続、再接続、席操作、取引操作、役割別描画、多言語表示 |
| `qomm_demo/static/demo.css` | レイアウト、状態表現、狭幅対応、操作部品 |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-demo/tests/model.rs` | 平文価格と勝者選択 |
| `rust/qomm-demo/tests/protocol.rs` | 正常、不正積、不正開示、不正入力、離脱、不足 |
| `rust/qomm-demo/tests/room.rs` | 席、権限、役割別秘密投影、ラウンド状態 |
| `rust/qomm-demo/tests/bots.rs` | 空席だけをbotが担当し、人の操作を奪わない |
| `rust/qomm-demo/tests/web.rs` | HTTP/WebSocket握手、メッセージ形式、静的配信 |

## 12. `qomm-law`: 配備条件の検査

### 製品コード

| ファイル | 責務 |
|---|---|
| `rust/qomm-law/src/lib.rs` | 日付、条文、要件、商品、判断、配備の型 |
| `rust/qomm-law/src/parse.rs` | `.law` の解析と出典行保持 |
| `rust/qomm-law/src/check.rs` | 未回答、証拠欠落、未施行、確認期限切れを拒否 |
| `rust/qomm-law/src/emit.rs` | 根拠・判断・証拠をMarkdownへ出力 |
| `rust/qomm-law/src/bin/law.rs` | lint、期限一覧、法域・商品・日付別コンパイルCLI |

### 試験

| ファイル | 固定する契約 |
|---|---|
| `rust/qomm-law/tests/law.rs` | 日付往復、施行、期限、義務、証拠、出力、行番号 |

## 13. `qomm-measure`: 測定の共通契約

| ファイル | 責務 |
|---|---|
| `rust/qomm-measure/src/lib.rs` | 標本数、平均、標準偏差、中央値、範囲、決定値の区別 |
| `rust/qomm-measure/src/fsum.rs` | 過去成果物と整合する補償和 |
| `rust/qomm-measure/src/deterministic_random.rs` | 固定済みの決定的乱数 |
| `rust/qomm-measure/src/rounding.rs` | 過去成果物と整合する丸め |
| `rust/qomm-measure/src/beta.rs` | Beta分布、二項比率区間等の数値処理 |
| `rust/qomm-measure/src/hosts.rs` | 公開用ホスト名と実行環境情報 |
| `rust/qomm-measure/src/bin/host_label.rs` | 現在ホストの公開ラベルを返すCLI |

## 14. `qomm-harness`: 実行、測定、成果物生成

### 共通モジュール

| ファイル | 責務 |
|---|---|
| `rust/qomm-harness/src/lib.rs` | リポジトリ位置、一時領域、外部コマンド、JSON、数値表示の共通処理 |
| `rust/qomm-harness/src/local_mpc.rs` | 一台上の複数MP-SPDZ参加者を準備・実行・観測 |
| `rust/qomm-harness/src/measure.rs` | ハーネス側の測定整形・互換処理 |
| `rust/qomm-harness/src/smallsample.rs` | 小標本を確証と誤認しない統計補助 |
| `rust/qomm-harness/src/defmi_cycle.rs` | DeFMIサイクル測定の共通fixture |
| `rust/qomm-harness/src/legacy_zk_bench.rs` | 旧暗号ベンチ契約との比較 |
| `rust/qomm-harness/src/voleith.rs` | VOLEitH比較用の符号・ハッシュ・詰込み処理 |
| `rust/qomm-harness/tests/fill_fold.rs` | ハーネスと回路のfill/key折畳み契約 |

### 文書、公開、成果物管理

| ファイル | 動作 |
|---|---|
| `rust/qomm-harness/src/bin/build_audit_doc.rs` | 成果物から`AUDIT.md`を決定的に再生成 |
| `rust/qomm-harness/src/bin/build_defmi_doc.rs` | 成果物から`DEFMI.md`を決定的に再生成 |
| `rust/qomm-harness/src/bin/make_figures.rs` | 測定JSONからSVG/PNG/PDF図を生成 |
| `rust/qomm-harness/src/bin/manifest.rs` | 成果物SHA-256台帳の生成・検査 |
| `rust/qomm-harness/src/bin/report.rs` | 主要成果物を一つの読取り用報告へ集約 |
| `rust/qomm-harness/src/bin/scrub_artifacts.rs` | 公開成果物から秘密・ホスト固有情報を規則に従い除去 |
| `rust/qomm-harness/src/bin/export_repos.rs` | QOMM/DeFMI/zkPI公開リポジトリをallow-listから構成し、依存・日本語漏洩を検査 |
| `rust/qomm-harness/src/bin/research_run.rs` | 契約・manifest・台帳へ要約を結び、許可された実験だけを起動 |

### データ収集

| ファイル | 動作 |
|---|---|
| `rust/qomm-harness/src/bin/collect_uniswapx.rs` | Ethereum RPCからUniswapX約定と移転量を収集 |
| `rust/qomm-harness/src/bin/collect_origins.rs` | 頻出取引主体の作成元・委任関係を解決し、候補群を作る |
| `rust/qomm-harness/src/bin/derive_snr.rs` | DP公開の信号対雑音比モデルを導出 |

### MPC回路・通信・配置

| ファイル | 動作 |
|---|---|
| `rust/qomm-harness/src/bin/run_qomm.rs` | 一つのQOMM回路を生成・実行し、平文正解、時間、通信を記録 |
| `rust/qomm-harness/src/bin/sweep.rs` | MM数・遅延・RFQ/RFM/RFS・公開方式を走査 |
| `rust/qomm-harness/src/bin/opt_sweep.rs` | ビット幅、比較木分岐、前処理、脅威モデルを走査 |
| `rust/qomm-harness/src/bin/run_stages.rs` | 価格、適格判定、比較木を段階停止して各層の費用を分離 |
| `rust/qomm-harness/src/bin/run_rounds.rs` | 回路のラウンド数・送信量を方式別に記録 |
| `rust/qomm-harness/src/bin/run_multiplication_cost.rs` | 秘密乗算の幅・深さ・遅延費用を測る |
| `rust/qomm-harness/src/bin/run_input_check.rs` | 入力後ランダム線形検査の正常・改変・費用を測る |
| `rust/qomm-harness/src/bin/run_identity.rs` | ノード別入力コミットメントと回路開示値の同一性を検査 |
| `rust/qomm-harness/src/bin/run_binding_chain.rs` | DSL監査、VSS入力、MPC、回路ワイヤ証明まで一本で結ぶ |
| `rust/qomm-harness/src/bin/run_circuit_bound_proof.rs` | MP-SPDZ永続ワイヤから共同価格証明を組み立てる |
| `rust/qomm-harness/src/bin/run_threshold_assembly.rs` | 共有範囲・価格証明の人数・幅・MM数別費用 |
| `rust/qomm-harness/src/bin/run_bitdec_rounds.rs` | ビット分解共同証明をMPCで組み立てるラウンド数 |
| `rust/qomm-harness/src/bin/run_distributed_assembly.rs` | ノードごとの独立OSプロセスで共同証明を作る境界を実測 |
| `rust/qomm-harness/src/bin/run_pretrade_reservations.rs` | Maker/Taker事前承認を検査し、CCP・銀行・自己保証枠と資産をAvalancheへ予約する |
| `rust/qomm-harness/src/bin/build_settlement_contexts.rs` | DeFMI予約受領証とMPCハンドオフから型付き決済文脈を作る |
| `rust/qomm-harness/src/bin/settle_finalized_batch.rs` | 最終共同証明を再検査し、受付順の複数RFQを一つのAvalanche商品バッチで決済する |
| `rust/qomm-harness/src/bin/run_transport.rs` | 固定フレーム、中継、実/ダミーの観測可能差を測る |
| `rust/qomm-harness/src/bin/run_audit_slots.rs` | 空/実スロットの同一形と、故障注入・受領記録を検査 |
| `rust/qomm-harness/src/bin/run_multi_asset.rs` | 資産数を増やし、秘密資産選択の回路費用を測る |
| `rust/qomm-harness/src/bin/run_maker_updates.rs` | RFS在庫更新回数と直列依存の費用を測る |
| `rust/qomm-harness/src/bin/run_three_times.rs` | MPC価格回答、別fixtureによる証明、監査受領記録を三時点で測る。三者の暗号的結合やDeFMI決済は行わない |
| `rust/qomm-harness/src/bin/run_serve_bench.rs` | cold、常駐、socket経由の見積もり時間を比較 |
| `rust/qomm-harness/src/bin/run_placement.rs` | 7ノード配置と遠隔一台が全体へ与える影響を測る |
| `rust/qomm-harness/src/bin/run_sites.rs` | 複数実ホストへ7参加者を割り当て、配布・トンネル・実行を統括 |
| `rust/qomm-harness/src/bin/tcp_rtt.rs` | 指定host:portへのTCP接続時間中央値を返す |
| `rust/qomm-harness/src/bin/run_robust_atlas.rs` | ATLAS復号で不正積断片を訂正・特定し、継続可能数を測る |
| `rust/qomm-harness/src/bin/run_voleith.rs` | Pedersen入力検査とVOLEitH型検査を時間・バイト・ハッシュ数で比較 |

### 証明・資格・探り耐性

| ファイル | 動作 |
|---|---|
| `rust/qomm-harness/src/bin/zk_bench.rs` | 匿名KYB、ポリシー監査、暗号最適化段階を測る |
| `rust/qomm-harness/src/bin/zk_compare.rs` | 候補証明方式を集合・値数ごとに比較 |
| `rust/qomm-harness/src/bin/run_quote_proof.rs` | 単独生成と共同生成の価格証明をMM数別に測る |
| `rust/qomm-harness/src/bin/issue_external_kyb.rs` | 受入用の外部KYB信頼アンカーと署名済みassertion束を安全なファイルへ発行する |
| `rust/qomm-harness/src/bin/qomm_hsm_signer.rs` | 受入用外部署名プロセス。PINファイルを安全に読み、CSD署名要求へ応答する。物理HSMの証拠ではない |
| `rust/qomm-harness/src/bin/run_state_audit.rs` | 在庫状態列の長さ別費用と全拒否条件を測る |
| `rust/qomm-harness/src/bin/run_probe_budget.rs` | 法人単位の正確見積もり取得・小ロット探り予算を評価 |
| `rust/qomm-harness/src/bin/run_entity_behavior.rs` | 複数資格の行動類似性を審査優先度として評価する煙試験 |
| `rust/qomm-harness/src/bin/run_disclosure_ceiling.rs` | 公開統計がMM判断を改善し得る理論上限を測る |
| `rust/qomm-harness/src/bin/run_block_range_query.rs` | 法人数等の範囲統計に必要な感度・予算・有用性を測る |

### DP・市場・攻撃実験

| ファイル | 動作 |
|---|---|
| `rust/qomm-harness/src/bin/run_distributed_dp.rs` | MP-SPDZ内で分散離散Laplaceを実行し分布・費用を検査 |
| `rust/qomm-harness/src/bin/run_distributed_publication.rs` | 7プロセスMPC、DP雑音、法人別予算正本、3-of-7公開証明を一走行で検査 |
| `rust/qomm-harness/src/bin/run_dp_audit.rs` | 二世界メンバーシップ実験で実装上のε下界を監査 |
| `rust/qomm-harness/src/bin/run_dp_effect.rs` | DP公開を入れた市場効果を実データ上で比較 |
| `rust/qomm-harness/src/bin/run_sim_matrix.rs` | 方式・公開・ε・seedの比較行列を並列実行 |
| `rust/qomm-harness/src/bin/run_rho_sweep.rs` | 情報を持つ利用者比率ρに対する結果感度を走査 |
| `rust/qomm-harness/src/bin/run_market_thickness.rs` | MM数と要求量に対する約定率差の煙試験 |
| `rust/qomm-harness/src/bin/run_staleness.rs` | 公開情報の遅れが価格・MM損益・有用性へ与える影響 |

### DeFMI・チェーン・比較環境

| ファイル | 動作 |
|---|---|
| `rust/qomm-harness/src/bin/run_defmi.rs` | DvP、ノート、リング、ネッティング、資産数、並列性を総合測定 |
| `rust/qomm-harness/src/bin/run_deccp.rs` | CCP更改とネット決済を取引数別に測る |
| `rust/qomm-harness/src/bin/run_reconcile.rs` | 原簿総数照合と差異探索を測る |
| `rust/qomm-harness/src/bin/run_viewing.rs` | 閲覧範囲、走査、許可、支出開示の費用を測る |
| `rust/qomm-harness/src/bin/run_avalanche_l1_acceptance.rs` | 5検証者L1で資産・口座・決済・再起動・状態根一致を検査 |
| `avalanche/defmivm/scripts/run-full-qomm-l1.sh` | 事前予約、実7者MP-SPDZ、共同zkPI、同時RFQ、商品DvP、投影停止窓、検証者再起動を一括受入 |

### 実演入口

| ファイル | 動作 |
|---|---|
| `rust/qomm-harness/src/bin/serve_demo.rs` | Rustだけでブラウザ実演を起動 |

## 15. 専用Avalanche VM

### Rust VM本体

| ファイル | 責務 |
|---|---|
| `rust/qomm-avalanche-vm/src/main.rs` | `vmid`、genesisコンパイル、AvalancheGoから起動される外部VMの入口 |
| `rust/qomm-avalanche-vm/src/lib.rs` | VMの版、モジュール境界、Protocol 45プラグイン起動を公開 |
| `rust/qomm-avalanche-vm/src/id.rs` | AvalancheのCB58 IDと固定32バイトIDを相互変換し、検査和を確認 |
| `rust/qomm-avalanche-vm/src/transaction.rs` | 許可された25種類のDeFMI取引だけを正規JSONへ符号化し、取引IDを決定 |
| `rust/qomm-avalanche-vm/src/genesis.rs` | 7委員、しきい値、公開鍵、時刻のgenesis検証と正規バイナリ化 |
| `rust/qomm-avalanche-vm/src/state.rs` | 資産、CSD、ノート、請求権、保証主体、法人保証枠、予約、受付順、使用済み値を含む正本状態と状態根 |
| `rust/qomm-avalanche-vm/src/execution.rs` | 25取引の合意決定的な状態遷移。旧状態、期限、承認、二重使用、枠超過、受付順、原子的一括決済を検査 |
| `rust/qomm-avalanche-vm/src/block.rs` | 親ID、時刻、高さ、最大32取引からなる正規ブロックとID |
| `rust/qomm-avalanche-vm/src/vm.rs` | RPCChainVM 45、mempool、Build/Verify/Accept/Reject、永続DB、再起動復元、JSON-RPC、確定状態スナップショット |

### AvalancheGoとの境界

| ファイル | 責務 |
|---|---|
| `rust/vendor/avalanche-rs-qomm/UPSTREAM.md` | `avalanche-rs`元コミット、AvalancheGo版、Protocol 45差分を固定 |
| `rust/vendor/avalanche-rs-qomm/LICENSE` | Ava Labs Ecosystem License 1.1の完全な原文 |
| `rust/vendor/avalanche-rs-qomm/crates/avalanche-rpcchainvm/src/plugin.rs` | VM側の待受を作り、AvalancheGo RuntimeへProtocol 45と待受先を通知してgRPCサービスを開始 |
| `rust/vendor/avalanche-rs-qomm/crates/avalanche-rpcchainvm/src/database.rs` | AvalancheGoが提供する正本DBサービスのRustクライアント |
| `rust/vendor/avalanche-rs-qomm/crates/avalanche-rpcchainvm/src/app_sender.rs` | VMからAvalancheネットワークへアプリケーション通知を送る境界 |
| `rust/vendor/avalanche-rs-qomm/crates/avalanche-rpcchainvm/proto/` | AvalancheGo v1.14.2へ固定したRuntime、VM、HTTP、DB、送信サービスのスキーマ |

AvalancheGo本体はフォークしない。AvalancheGoがRust実行ファイルを別プロセスとして起動し、ローカルgRPCで呼び出す。QOMM所有の合意状態機械はRust VMだけである。

### 試験・運用入口

| ファイル | 固定する契約または動作 |
|---|---|
| `rust/qomm-avalanche-vm/src/execution.rs`内試験 | Maker/Taker事前予約、事後署名なし決済、同時RFQ、法人合算枠、7ノード受付順、口座なしノートDvP、CSD署名、再利用拒否 |
| `rust/qomm-avalanche-vm/src/vm.rs`内試験 | JSON-RPC、ID、保存キー、エラーコード、Protocol 45のVM動作 |
| `rust/vendor/avalanche-rs-qomm/crates/avalanche-rpcchainvm/src/plugin.rs`内試験 | Runtime初期化通知がProtocol 45と実際の待受先を送り、外部アドレスを拒否すること |
| `avalanche/defmivm/config/test-genesis.json` | 7公開鍵・3-of-7の決定的な試験genesis |
| `avalanche/defmivm/scripts/run-local-l1.sh` | 5 AvalancheGo、資産・口座互換・決済、同一取引回復、検証者再起動、状態根一致 |
| `avalanche/defmivm/scripts/run-full-qomm-l1.sh` | 外部KYB、外部CSD署名、実7者MP-SPDZ、口座なし予約、法人合算枠、共同zkPI、2 RFQ原子決済、請求権、再起動復元 |

## 16. Rust補助測定コード

| ファイル | 位置づけ |
|---|---|
| `solana/cu_probe/src/lib.rs` | Solana上の検証計算単位を測るprogram |
| `solana/runner/src/main.rs` | programへ測定取引を送るrunner |
| `stylus/probe/src/lib.rs` | Stylus上の決済検査関数 |
| `stylus/probe/src/main.rs` | Stylus probeのnative入口 |
| `stylus/measure.sh` | 配備・呼出し・gas結果を成果物化 |

これらはRust製の補助測定であり、現行の配備先ではない。

## 17. その他の実行スクリプト

| ファイル | 動作 |
|---|---|
| `scripts/run_two_site.sh` | 二拠点へMP-SPDZ参加者を配置し、実RTTでQOMM回路を実行 |

## 18. 外部MP-SPDZとの境界

QOMM所有の回路生成、実験、編成、検証はRustである。外部MP-SPDZの公式コンパイラだけは、その配布形式のまま利用する。リポジトリ内に別実装や参照用スクリプトは置かない。

| ファイル | 責務 |
|---|---|
| `rust/qomm-mpc/src/program.rs` | 承認済み設定から決定的なMP-SPDZ入力を生成 |
| `rust/qomm-mpc/src/compiler.rs` | 指定された外部checkoutが公式コンパイラ構造を持つことを検査して起動 |
| `rust/qomm-harness/src/local_mpc.rs` | 公式コンパイラを呼び、リンクしたMP-SPDZエンジンを参加者別Rustプロセスとして編成する |
| `rust/qomm-mpc/shim/qomm_spdz.cpp` | 必要な場合だけ`libSPDZ`をRustから呼ぶ狭いC ABI |
| `rust/qomm-transport/src/bin/seven_node_cluster.rs` | 7つの独立Rustプロセスを編成し、外部MP-SPDZを7 OSプロセスとして実行 |
| `rust/qomm-transport/src/bin/qomm_node_party.rs` | 各参加者の秘密入力、固定通信、受領記録、結果を扱うRust入口 |
| `rust/qomm-harness/src/rust_only.rs` | QOMM所有の別言語実装・実験入口を拒否し、外部公式コンパイラだけを例外として検査 |

## 19. 実行を左右する主な非コードファイル

| ファイル・ディレクトリ | 役割 |
|---|---|
| `rust/Cargo.toml` | 13クレート、依存版、release設定、共通feature |
| `deploy/node.example.json` | KYBを含む常駐受付ノードの設定例 |
| `deploy/proof-party.example.json` | 証明・FROSTノードの相互TLS、秘密状態、鍵庫の設定例 |
| `deploy/approved-programs.example.json` | 現行の許可プログラム、要約、引数制約の設定例 |
| `deploy/wan/` | 7ホストの配置、秘密分離、相互TLS、受入手順。実7ホストでの実行は別途必要 |
| `rust/qomm-dsl/examples/*.rule` | 承認可能・拒否される価格規則例 |
| `rust/qomm-law/rules/*.law` | 法域・商品・条文・証拠の入力 |
| `research/contract.json` | 研究目的、段階、禁止された近道、確証条件 |
| `research/manifests/*.json` | 個々の実験の仮説、対象、予測、判定規則 |
| `artifacts/MANIFEST.json` | 測定成果物のSHA-256台帳 |
| `avalanche/defmivm/config/test-genesis.json` | ローカル受入試験の委員会genesis例 |

## 20. 一つの変更が影響する場所

| 変更 | 同時に確認するファイル |
|---|---|
| 価格式を変える | `qomm-dsl`、`qomm-mpc/program.rs`、`qomm-proofs/rule_audit.rs`、`quote_proof.rs`、`qomm-sim/market.rs`、`qomm-demo/model.rs` |
| MM入力項目を増やす | `qomm-mpc/program.rs`、`inputs.rs`、`qomm-proofs/policy_audit.rs`、DSLの宣言、入力検査試験 |
| フレーム形式を変える | `wire.rs`、`client.rs`、`relay.rs`、`node_service.rs`、`run_transport.rs`、wire試験 |
| zkPI項目を変える | `qomm-zkpi/lib.rs`、`wire.rs`、`wire_vectors.rs`、`qomm-defmi/settlement.rs`、Avalancheのstatement互換試験 |
| DeFMI遷移文を変える | `qomm-defmi/facility.rs`、`qomm-defmi/avalanche.rs`、`qomm-avalanche-vm/execution.rs`、取引互換試験 |
| 状態根を変える | `qomm-defmi/facility.rs`、`qomm-avalanche-vm/state.rs`、受入試験、既存genesis互換方針 |
| 委員会承認文を変える | Rust `QuorumAuthorizer`、Rust VMの取引検査、Chain ID領域、全負試験 |
| DP公開を変える | `qomm-audit/distributed_dp.rs`、`publication.rs`、`qomm-sim/disclosure.rs`、二世界監査、予算試験 |
| ブラウザ表示を変える | `room.rs`の役割別投影、`web.rs`、`demo.js`、`demo.css`、実ブラウザ確認 |

この対応を崩す変更は、回路と証明、研究モデルと製品状態機械のどれかをずらす可能性が高い。
