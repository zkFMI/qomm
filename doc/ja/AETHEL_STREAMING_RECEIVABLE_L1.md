# Aethel: ストリーム債権・与信プロバイダ・保証をAvalanche L1へ載せる設計

## 結論

Aethelは金融機関でも、単一の与信エンジンでもない。支払ストリームから将来債権を作り、分割し、
DeFMI上で流通・回収できるようにするプロトコルである。参加企業は、KYC/KYB credential発行、審査、
保証、前払資金、ストリーム事実の証明、回収管理の一部または複数を提供できる。同一企業が複数の能力を
持ってもよいが、プロトコル上の権限と義務は別々に登録する。

特に、次の三つを同一視しない。

- **Credit decision**: リスク判断という情報。資金提供や損失負担の約束ではない。
- **Guarantee commitment**: 不履行時に損失を負担する契約。DeFMIの保証枠と予約を必要とする。
- **Funding quote**: 債権取得時に前払資金を出す約束。DeFMIの現金予約を必要とする。

この分離は過去のAethel検討で定義された「スコアリング（情報）」と「リクイディティ（資金）」の
分離を維持しつつ、今回必要になった「保証（偶発債務・リスク資本）」を第三の義務として明示する。

## コンポーネント境界

| コンポーネント | 正本として持つもの | 持たないもの |
|---|---|---|
| Aethel Core | provider capability、匿名subject binding、署名済みstream state、receivable series、credit decision、guarantee/fundingの意味的結合、回収・claim状態 | 法的氏名・法人番号、現金残高、債権所有権、保証枠残高、秘密モデルの中身 |
| zkPI | KYC/KYB credentialの所持とscope nullifier、秘匿金額の範囲、発行額と`pledged`の加法関係、発行後残余枠、stream state、provider artifact、DeFMI backing、期限の同一性証明 | issuerによるKYC/KYB判断の正しさ、入力データの真正性、汎用的な秘密モデル実行証明、モデル品質、法的譲渡の有効性 |
| DeFMI | KYC/KYB issuerを含む参加者role・用途別鍵、債権note、現金note、DvP、保証facility/hold、cash reservation、settlement claim、二重使用防止 | 匿名subjectの法的氏名、与信判断、stream covenantの業務的意味 |
| Avalanche L1 VM | 三者の状態遷移順序、k-of-n承認、zkPI検証鍵epoch、state root、replay拒否 | オフチェーン審査、法的回収、法定通貨off-ramp |
| 外部provider | KYC/KYB原本とcredential、source data、審査model/policy、保証契約、資金、servicing判断 | Aethel/DeFMIの正本状態を単独で書き換える権限 |

`aethel-core`を独立crateにした理由は、Aethel固有のstreaming-receivable semanticsをDeFMIへ混ぜず、
DeFMIとzkPIをAethel以外でも利用可能な状態に保つためである。

## Provider capability

`ProviderDefinition`は検証済みDeFMI participantを参照し、次の能力を個別に登録する。

1. `stream_attestor`: payer、payee、rate、pause/cancel、paid、arrears等のsource eventを署名する。
2. `credential_issuer`: 法的主体をオフチェーンでKYC/KYBし、scope別の匿名subject commitmentへ署名する。
3. `credit_assessor`: 登録済みmodel/policyからprivate credit decisionを作る。
4. `guarantor`: claim条件とcoverageを約束し、DeFMI guarantee facilityからholdを作る。
5. `liquidity_provider`: executable funding quoteを出し、DeFMI cash noteを予約する。
6. `servicer`: default/arrears/collection eventを契約上の権限範囲で署名する。

providerの公開鍵はDeFMI participantの用途別quote keyに一致させる。保証能力を登録するproviderは、
同じ鍵で登録されたDeFMI guarantor IDも持つ。Aethel自身がreference providerを運用する場合も、
この同じ登録経路を使い、特権的な内蔵providerにはしない。

## 秘匿主体とKYC/KYB issuer（DeKYX）

匿名主体の本人確認は、Aethel固有の機能ではなく独立基盤DeKYX（`mvp/dekyx`）が所有する。
`aethel-core`は`dekyx-core`と`dekyx-aethel`へ依存し、Aethel側に残るのは次の薄いadapterだけである
（`rust/aethel-core/src/subject.rs`）。

- どのproviderがどのDeKYX issuer鍵を保証するか（`RegisterCredentialIssuer`）。DeFMIでは
  `ParticipantRole::CredentialIssuer`、Aethelでは`ProviderCapability::CredentialIssuer`を持つproviderが、
  自分のprovider鍵で署名してDeKYX issuer定義（issuer ID = provider ID、key epoch、DeKYX署名鍵、namespace、
  対象主体種別、有効期間）を登録する。DeKYX署名鍵はprovider鍵と別で、providerが同じIDで上位epochを
  登録すれば回転する。`previousEpochsValidUntil`が旧epochの猶予期限で、旧epochの`validFrom`より前を
  指定すると即時失効（鍵漏洩経路）になる。
- issuerが署名した失効リストの公開（`PublishCredentialStatus`）。epochごとに一つ、status epochは単調増加で、
  リストの無いepochの credentialはfail-closedで検証できない。
- 主体のpresentationをどのartifactへ束縛するか（`ConfidentialArtifact`）。scope = `requestId`、
  audience = `aethelDomainId`、action = credit decision / guarantee、request = 未署名artifactのstatement digest、
  nonce = artifactのnonce、期限 = artifactの期限。

処理は次の境界で行う。

1. 対象主体は高エントロピーの`subjectSecret`と乱数からrequest scope専用のPedersen commitmentを作り、
   その開示知識をZKで示してcredential発行を求める。issuerは秘密も乱数も受け取らない。
2. issuerだけが法的主体との対応をオフチェーンで確認し、issuer ID/key epoch、`requestId`、eligibility policy、
   資格属性のMerkle root、有効期限を含むcredentialへ署名する。氏名、法人番号、issuer内部顧客IDはcredentialへ
   入れない。
3. 対象主体は、credit decisionまたはguaranteeごとに、そのartifactへ束縛したfresh transcriptで、
   credentialのcommitmentとscope別nullifierが同じ秘密を含むことをRistretto/Schnorr proofで示し、
   seriesが要求する資格属性（例: `jp.kyb`）だけを選択開示する。
4. Avalanche VMはprovider/participantの実行時状態だけを確かめ、検証は`aethel-core`経由でDeKYXが行う。
   DeKYXはissuer鍵epochの有効性、credential署名、失効リスト、資格属性proof、ZK proof、context一致を検査し、
   `AethelSubjectBinding`を返す。Aethelはその記録だけをrequest scopeごとに保持し、同じscope pseudonymと
   contextの組をreplay台帳で一度しか受け付けない。
5. credit decisionとguaranteeは同じ`requestId`のbindingを参照する。subject line ID =
   H(issuer, scope, policy, nullifier)で、issuer鍵の回転や再発行があっても同じ秘密なら同じ枠として
   結ばれる。issuer provider ID、credit assessor provider ID、guarantor provider IDは別々でよい。
6. confidential seriesの発行時は、credit decisionの秘匿枠commitmentとzkPIの`eligibleCommitment`を一致させ、
   `pledgedAfter = pledgedBefore + amount`と残余非負を証明する。したがって公開額を出さずに累積枠を消費できる。

| 情報 | Credential issuer | Credit assessor / guarantor | L1観測者 |
|---|---|---|---|
| 法的氏名・法人番号との対応 | 知る | 原則知らない | 知らない |
| request scope、issuer、policy、期限、開示された資格属性 | 知る | 知る | 知る |
| scope内で同じ匿名枠か | 照合可能 | nullifierで照合可能 | nullifierで照合可能 |
| 別scope間で同じ主体か | issuer内部記録で照合し得る | 暗号学的には照合不能 | 暗号学的には照合不能 |
| 与信モデル入力 | issuerとの契約次第 | ZK/MPC化した入力だけなら原本不要 | 知らない |

同一scopeでは、credit decision、guarantee、債権発行を一つの匿名枠へ安全に結ぶため、意図的にlinkableである。
別scopeではcommitmentを再乱数化しnullifier baseも変えるため、離散対数仮定の下で公開情報からはlinkできない。
`subjectSecret`を氏名や法人番号から導出してはならない。

## 保証枠の正本はDeCCP

保証枠の予約・束縛・解放・消費の正本状態は、Aethelでもこの VM でもなく、独立基盤DeCCP（`mvp/deccp`）の
`ClearingBook`が持つ。VMはその帳簿を`State.deccp`として保持し、次の三つのDeCCP transactionで組み立てる。

- `defmivm.issueDeccpClearingBook`: DeCCP authority set（VM委員会とは別の閾値署名者集合）とCCP自己資本を登録する。
  自己資本の裏付けは、資本用lock tagで施錠したDeFMI cash noteである。
- `defmivm.issueDeccpMember`: Aethelのguarantor providerをclearing memberとして受け入れる。身元証拠は
  clearing membership scope向けのDeKYX presentationで、DeCCPが記録するのはsubject lineだけである。法人名も
  DeFMI participant registryの法人credential commitmentもDeCCPには渡らない。default fundはmember固有のlock tagで
  施錠したcash noteで裏付ける。
- `defmivm.issueDeccpGuaranteeFacility`: DeFMI credit facilityを同じIDの秘匿facilityとしてDeCCPに登録する。
  容量はcap commitment、受益者はDeFMI facilityのbeneficiary commitment、開始状態digestはcapとcollateralの
  commitmentから決定的に導く。金額の平文はどこにも現れない。

以後、`issueAethelGuarantee`はDeCCPへの予約（`AethelDeCcpAdapter::reserve`）、`issueAethelReceivable`は束縛、
`issueAethelGuaranteeClaim`は消費、新設の`issueAethelGuaranteeRelease`は解放を、それぞれAethel記録と同じ
transactionで原子的に行う。DeCCPが要求する`DeFmiPort`はVMのfacility/hold/note recordの上に実装し、
hidden capacityの状態digestは「前状態、遷移種別、DeFMI hold ID、amount commitment、DeFMI sequence」の
hash chainとして進める。DeCCPのCASはこの鎖に対して働き、金額を開く必要が無い。DeFMI側のhold検査
（facility・rail・commitment・有効期限）とzkPI検証は従来どおり省略しない。

帳簿の永続化は`ClearingSnapshot`の形でVM state（state rootに含まれ、decode時にcanonical encodingを再検査する）に
乗せ、`ClearingBook::restore_authenticated`で復元する。復元時にもDeCCPの構造不変条件を全て再検査する。
DeCCPが単独で使う場合の`restore`（authority quorum署名を要する）はそのまま残る。

## ストリーム状態

ストリーム状態は次をcommitmentで保持する。

```text
StreamState = (
  streamId, payer, payee, settlementAsset,
  sourceDomain, terms, eventRoot,
  accrued, paid, eligible, pledged,
  asOf, version, status
)
```

外部の`stream_attestor`がsource evidenceと遷移proofへ署名する。Aethel Coreはversionの連続性、
不変フィールド、時刻、状態遷移を検査する。`pledged`だけは債権発行zkPIによりAethelが更新し、
外部attestorが後から減らしたり消したりできない。

Phase 1では署名付きstream/payment eventを入力とする。SuperfluidはEVM側のsource adapterとして接続できるが、
非EVMの専用Avalanche L1ではAethelのnative stream stateが正本であり、Superfluid固有contractを
VM内部へ移植しない。

## 債権化と流通

`ReceivableSeries`はstream、債権asset、issuer、maturity、policyを束縛する。policyは、審査、保証、
資金予約を必須にするか、無保証で流通可能にするかをseriesごとに決める。private thresholdや
concentration limitはpolicy digestへcommitし、zkPIで証明する。

発行時は以下を同じtyped zkPIへ入れる。

```text
requestId
seriesId / streamId / streamStateVersion
beforeStreamRoot / afterStreamRoot
DeFMI receivableNoteId / settlementAssetId
creditDecisionId + provider + proof binding       (optional)
guaranteeId + provider + DeFMI holdId             (optional)
fundingQuoteId + provider + DeFMI cashReserveId   (optional)
eligibleCommitment / pledgedBefore / pledgedAfter
eligibilityPolicyDigest / eligibilityRangeProofDigest
allocationNullifier / beforeAethelRoot
```

基礎zkPIのamount commitmentは債権額である。consideration commitmentは、資金予約があればadvance、
保証だけならcoverage、どちらもなければ債権額に一致させる。Avalanche VMはthreshold range proofと
FROST authorizationを、governanceが固定したverifier epochで検証する。さらに
`pledgedAfter = pledgedBefore + amount`をcommitment上で検査し、`eligible - pledgedAfter`が
非負かつ公開されたamount bit幅内であることを、別のthreshold range proofで検証する。
そのproof bytesのcanonical digestを基礎zkPIとAethel contextの両方へ束縛するため、単なる
「proofがある」という参照だけでは発行できない。

`FundingQuote`を付けた発行は、発行時点でもDeFMI cash reservationがactiveであることを再検査する。
ただし、この時点を「前払済み」とは扱わない。実際のadvanceは既存DeFMI DvPでcash reservationを消費し、
receivable noteとの両脚receiptが確定した時点でのみ支払済みになる。Aethel発行だけで現金移動を推定しない。

発行済み権利はDeFMI noteであり、分割、予約、二次売買、DvP、claim materializationには既存の
DeFMI note pathを使う。Aethel内に別のDEX台帳や所有権台帳を作らない。各seriesには一意の
receivable asset IDを割り当てる。`allowSecondaryTransfer=true`なら発行noteをunlockedで作り、
通常のDeFMI note spend/DvPで同じasset IDを保存したまま移転できる。`false`ならseries IDでlockし、
通常移転経路から消費できない。

## 保証とclaim

`GuaranteeCommitment`は次を必須にする。

- guarantee providerと署名
- 対象request、series、versioned stream state
- coverage commitmentとloss layer
- claim policy、expiry、nonce
- DeFMI guarantor、facility、active hold

VMはfacilityのguarantor、settlement asset、期限と、holdのamount/statusを確認する。したがって、
有効なcredit decisionだけでは「保証付き」と表示できない。

claim時はservicerの`DefaultAttestation`、claim zkPI、消費済みDeFMI guarantee hold、実際の
settlement digestを結合する。さらに同じhold、cash asset、amount、recipient、settlement digestを持つ
DeFMI `delivery` entitlementが存在しなければclaimを確定しない。ZK proofだけ、default oracleだけ、
provider署名だけでは支払済みとしない。同じsettlementには、そのseries固有asset IDのreceivable noteを
匿名spendしたDeFMI serialも必要なので、二次取得者へ移った債権でも現保有者の権利行使と保証支払を
同じfinal settlementへ結べる。
回収後の代位・waterfallはclaim policyで定義し、DeFMI note/claimへ接続する。
現在のMVPは過大claimを避けるため、holdのcoverage commitmentと同額のfull-cover claimだけを受け付ける。
partial claimを導入する場合は、coverage残高のcommitmentと残余range proofを追加する。

## Avalanche L1 transaction

実装したconsensus methodは次である。

```text
defmivm.issueAethelProvider
defmivm.issueAethelStream
defmivm.issueAethelStreamTransition
defmivm.issueAethelSeries
defmivm.issueAethelCredentialIssuer
defmivm.issueAethelCredentialStatus
defmivm.issueAethelCreditDecision
defmivm.issueAethelGuarantee
defmivm.issueAethelFundingQuote
defmivm.issueAethelReceivable
defmivm.issueAethelDefault
defmivm.issueAethelGuaranteeClaim
defmivm.issueAethelGuaranteeRelease
defmivm.issueAethelProviderKeyRotation
defmivm.issueAethelProviderStatus
defmivm.issueDeccpClearingBook
defmivm.issueDeccpMember
defmivm.issueDeccpGuaranteeFacility
```

全methodは、既存DeFMI transactionと同じく`expectedBeforeRoot`とk-of-n承認を必要とする。
DeCCP memberとfacilityはさらにDeCCP authority setの承認（`deccpApproval`）を要する。

provider鍵のrotationは、DeFMI participant registryがadmin鍵で先にquote鍵を回してから、Aethel側で
`RotateProviderKey`（次鍵による所持証明署名付き）を流す順である。VMは次鍵がparticipantの現在のquote鍵と
一致することだけを確かめ、退役鍵は`retiredKeys`に残す。退役鍵で署名済みの記録はそのまま有効で、
退役鍵による新規artifactは拒否される。guarantor providerのDeFMI guarantor recordは登録時の鍵に結び付いたままで、
rotationでは再結合しない。`SetProviderStatus`はsuspend/reinstate/revokeで、revokeは終端である。
Aethel bookはAvalanche state rootへ含まれ、operation ID、provider nonce、allocation nullifier、note ID、
hold ID、cash reservation IDを再利用できない。

## 信頼境界

- 現在のzkPIは、発行額・`pledged`・残余eligible枠の関係を証明し、credit decisionのmodel/policy/proof digestを
  実行へ束縛する。秘密モデルを正しく実行したこと自体はまだ汎用検証せず、必要なproviderには
  model-specific verifier/circuitを追加する。予測力、公平性、校正は別のgovernance課題である。
- DeKYX presentationは、登録issuer鍵epochのcredential所持、失効リストに無いこと、要求された資格属性の包含、
  commitment/nullifierが同じ秘密へ束縛されること、そしてそのartifactへの束縛を証明する。issuerがKYC/KYBを
  正しく行ったことや属性が真実であることまでは証明しない。credentialのissuer署名は発行記録と照合し得るので、
  issuer-unlinkableな匿名credentialではなくscope-pseudonymousである。
- KYC/KYB issuerは法的主体との対応を知る。issuerとcredit assessor/guarantorの共謀、APIメタデータ、時刻、
  金額や取引グラフからの推測まで防ぐ匿名化ではない。原本を審査者へ渡せば、この秘匿境界は失われる。
- scope nullifierが防ぐのは同じ`subjectSecret`の再利用である。別secretへの重複credential発行はissuer側の
  一意性管理に依存する。複数issuerを同時に認めるpolicyには、共有重複防止registryまたはVOPRF等の
  issuer横断anti-Sybil方式が必要であり、現在のMVPには含まれない。
- stream sourceの真正性はattestor signature、監査、API/TLS証跡等に依存する。ZKだけでは作れない。
- Aethel/DeFMI内のnullifierは同一protocol内の二重債権化を防ぐ。外部登記や法的譲渡登録外の二重譲渡は、
  法務・registry連携が必要である。
- 将来キャッシュフローの取消可能性、recourse、seniority、倒産隔離、lender of recordはjurisdiction別policyと
  契約で定める。暗号学的finalityだけで法的finalityを主張しない。

## 現在の実装範囲

- `mvp/dekyx`（DeKYX）: issuer directory（鍵回転、失効リスト、検証付き再読込）、KYC/KYB credential、資格属性の選択開示、scope別Schnorr presentation、replay台帳、Aethel adapter。
- `aethel-core`: DeKYX issuer鍵の保証登録と失効公開、artifact束縛context、provider（鍵rotation・状態制御付き）、stream、series、decision、guarantee（release付き）、funding、issuance、default、claim状態機械。
- `qomm-zkpi`: streaming receivable用typed contextと固定wire format。旧`confidential_subject`は削除済みで、KYB/匿名主体はDeKYXだけが実装する。
- `mvp/deccp`（DeCCP）: 独立したclearing/保証枠core（zkPI検証必須、DeFMI receipt必須、三つのnetting mode、担保・証拠金・default waterfall、秘匿保証枠CAS、承認付きsnapshot復元とhost認証済みsnapshot復元）とAethel adapter。VMの保証経路はこのadapter経由に切替済み。
- `qomm-defmi`: credential issuer参加roleとspecialist credit providerを追加し、既存facility/hold/note/DvPを再利用。
- `qomm-avalanche-vm`: Aethel state rootとDeCCP clearing book、18種類のtransaction、DeFMI backingとzkPIの結合検査、DeKYX/DeCCP port実装。

production前には、法的assignment registry、provider onboarding/governance、実データsource attestation、
回収waterfall、外部cash railとのfailure atomicity、実組織・実拠点validatorでのacceptanceが別途必要である。
