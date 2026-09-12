# ブラウザからの暫定結果とnative決済

## 対象と現在の境界

QOMMの共同証明と暫定方式は、同じ7プロセスの秘密計算とDeFMI native台帳を使う。暫定方式で省略するのは通常時のquote計算証明であり、資金の証明、注文・計算結果の束縛、native finalityは残す。challengeが届いた場合は、保持した当該実行のwitnessから元のquote証明を生成する。

これは2026-09-12の統合作業ツリーを、隔離したSoftbank L40Sネットワークでブラウザ操作した記録である。未公開のUI改修を含む観測であり、この文書のcommitがその実行バイナリの配備を意味するものではない。参加者・資産・担保は研究用の合成データ。性能や本番運用の受入れ判定はしていない。

## 画面と実行経路

1. 検証方式で「共同証明」または「暫定方式」を選ぶ。処理中や企業要求の保持中は切り替えない。
2. 買い手が数量と指値を入力して送信する。企業outbox、暗号化入力、7プロセスのMPCを通る。
3. 暫定方式では、政策ID・実行context・出力commitment・提案者署名をnative台帳へ登録する。
4. Pendingの価格・数量は当該注文者のWebSocketにだけ加える。公開進捗にはclaimの状態と期限だけを載せる。
5. challengeなしなら期限後にFinalizedへ進む。challengeありなら元のquote証明を生成・検証する。正しい応答でも元のchallenge期限までは決済しない。
6. Finalizedと決済済みは別の状態。既存の金融証明とnative受渡しが完了し、readbackを確認してから残高を更新する。

WebSocketの受信処理は実行スレッドから分離しているため、計算中にもchallenge要求を受け取れる。注文者の暫定値は共有の公開snapshotに入れず、セッションの立場に応じて送る。

## 接続に必要な設定

既存の `demo-network/compose.yaml` のnativeネットワークと参加者構成を使用する。暫定方式を有効にするには、同じchain上でpolicy登録と提案者・challengerの担保拠出を先に完了し、そのpolicy IDをgatewayへ渡す。未登録policyをUIだけで有効化することはできない。

| gateway環境変数 | 意味 |
|---|---|
| `QOMM_ASSURANCE_STATE` | 選択方式を保存する書込可能な永続JSONファイル。親ディレクトリを事前に作り、コンテナ再作成をまたぐvolumeに置く |
| `ZKPI_ASSURANCE_MODE` | 保存済み選択がない場合の初期方式。通常は `joint_proof`、明示時だけ `optimistic` |
| `ZKPI_OPTIMISTIC_POLICY_ID` | 当該native chainへ登録済みの32byte policy IDのhex |
| `ZKPI_OPTIMISTIC_PROPOSER_PARTY` | 提案するMPC party。1始まり、既定は1 |
| `QOMM_PUBLIC_DEVELOPMENT_CHALLENGE=1` | 研究用challenger鍵で操作するボタンを有効化する明示的なfixture設定 |

最後の設定は、公開済みの決定的な研究用鍵を使う。実資産や外部利用者向けの認証・担保管理を提供する設定ではない。gatewayの方式JSONだけでなく、企業outbox、MPCの保持witness、native状態もそれぞれ保持する必要がある。

起動・ビルドはリポジトリ既存のnative構成に従い、Rustビルドと統合実行はSoftbank L40Sで行う。今回の接続確認では専用compose project `zkpi-opt-qomm-r10` とSSH tunnelを使用し、既存Omenデモを変更していない。gateway/frontendは `qomm-optimistic:demo-r4`、台帳は `qomm-optimistic:defmi-r12` で観測した。

## 期限と失敗の意味

challengeは元の期限より前、応答は応答期限より前だけ受け付ける。正しい一致証明はProvenへ進み、challenger担保を提案者へ移す。検証に通るが出力が矛盾する証明、または応答期限切れはRejectedとなり、提案者担保をchallengerへ移す。不正な証明bytesを送っただけでは即時に解決せず、検証エラーとなる。

未確定claimからの金融決済は拒否する。gatewayの表示エラーや接続断を、native台帳での取消しと解釈しない。再送・復旧時は保持要求と台帳を照合する。

## 実際の観測

同じUSDJPY注文、数量1・指値159.07に対し、約定価格は157.18だった。画面の円残高は内部金額単位の表示変換を含む。

| 経路 | 円残高の前後 | 在庫の前後 | 画面のreceipt |
|---|---|---|---|
| 共同証明 | 2,921,406 → 2,905,688 | 305 → 306 | `3308161fc1d388e0ba10d55384f24d0d314e1e2f7289c542f6c3e76cd9cf3a81` |
| 暫定・challengeあり | 2,905,688 → 2,889,970 | 306 → 307 | `20d0e5cc757575790fc07401d38e687a4af0e407c0c4c339b35007b24cf10656` |
| 暫定・challengeなし、390px幅 | 2,889,970 → 2,874,252 | 307 → 308 | `22f7a23a0cfbaab9282de4abbd3c0f517a4d876af553ec2a0cff09f31197334f` |

challengeありのclaimは `4054041bac7394c0b4cf372cc5828392671f9b4bfa08cbb26ce191f96ae0c176`。native readbackはheight 210、root `f1b3806b2a191f35e0f35ea6f35d0f4160ae710a89fe3050eeda3210f0998233`、Finalizedのproof digestありだった。

390pxの注文者画面で「暫定価格157.18 × 1」が表示された同じ時点に、別タブの公開ビューには期限だけが表示された。残高反映まで実際の送信操作を行った。証跡JSONは [verification](verification/OPTIMISTIC_BROWSER_20260912.json) を参照する。

## 関連する実装と解説

- [gatewayと方式設定](../rust/qomm-demo/src/distributed_mpc.rs)、[WebSocket](../rust/qomm-demo/src/web.rs)、[進捗](../rust/qomm-demo/src/progress.rs)
- [共通protocol](https://github.com/zkFMI/zkpi/tree/main/rust/zkpi-optimistic)
- [技術解説：暫定方式](https://zkfmi.com/ja/docs/optimistic.html)、[暗号技術37項目](https://zkfmi.com/ja/docs/crypto-catalog.html)

単一ホスト上の研究用ネットワークであり、独立組織のchallenger可用性、担保額の経済合理性、DoS耐性、長期間の復旧運用は未確認である。

## 公開依存版の接続と再実行（2026-09-13 JST）

一時的な隣接リポジトリへのpatchを削除し、公開済みのzkPI、SDK、暗号ポリシー、DeKYX・DeCCPを固定した。workspace内のqomm-law自己参照は従来どおり残す。Dockerfileもnative VMのビルド元を固定し、追加のresearch依存が未配置のcheckoutを要求しない版へ更新している。アプリ実装には進捗、React Flow、狭い画面、完了時の席解放と注文結果の保持を含む。

Composeのgatewayは方式をgateway-control volumeへ保存する。暫定方式には登録済みpolicyが必要で、空のpolicy IDではUIから選択できない。環境変数を指定してgatewayを再作成すると、同じchainに登録したpolicyを使える。既定の初期方式はjoint_proof、研究用challengeボタンは明示的に有効化する。

policy登録・担保拠出は [optimistic-policy example](../rust/qomm-demo/examples/optimistic-policy.rs) の --public-development を指定してnative RPCへ行う。これは公開開発鍵と合成担保を使う隔離ネットワーク専用の操作であり、通常起動時に資産やpolicyを勝手に作る処理ではない。

claimの待機・challenge応答を金融証明より先に処理する順序へ修正した。金融証明の生成時間がchallenge応答期限を消費しないためである。暫定結果の表示時点では未決済で、Finalized後にも金融証明・native受渡し・残高照合が必要になる。

実際のブラウザ操作で、共同証明の数量1・上限159.07は157.18で決済し、2,858,534円・309単位へ反映した。最終候補では390px幅で暫定価格157.18×1を確認してchallengeを送信し、元のquote証明が検証された。receipt 4990b0604145c08dffe2a2ad62267e59682e45dca247df3d322d02a8c14c95e7、残高2,811,375円・312単位を確認した。

同じ最終候補でchallengeなしの数量1も実行し、期限後に157.20で決済、2,795,655円となった。5台のnative readbackは、これらの実行後にheight 295、root 746ddd4c4b95960921f88abcc388eacca600403cff67d2196766e1aa7a1ab8f0で一致した。challengeありのFinalizedはproof digestあり、なしのFinalizedはproofなし。版、入力、receipt、claim、バイナリhashは[公開依存版の検証記録](verification/OPTIMISTIC_BROWSER_RELEASE_20260913.json)にまとめた。

画面は1981px幅と390px幅で確認した。注文入力、未決済表示、challenge送信、完了後のreceipt・残高を実際に操作し、狭い画面のパネル下部へスクロールして読めることを確認した。独立組織の運用・本番資産・性能優位性・包括的アクセシビリティ適合を示す記録ではない。
