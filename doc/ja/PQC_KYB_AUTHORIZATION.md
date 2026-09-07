# KYB 発行者認証と復元の境界

cohort registry と外部 KYB assertion は Ed25519 + ML-DSA-65 の両署名を必須とする。信頼する発行者鍵は登録済みの 1,984 byte 公開鍵であり、メッセージが提示した PQ 鍵を古典公開鍵だけで自己承認する経路はない。署名は共通 `zkfmi-crypto` のハイブリッド suite、用途 `Attestation`、3,373 byte を使う。cohort の本文 domain は v2、外部 assertion/bundle は v3。法的支配グループ識別子の既存 domain は保持し、鍵の変更で経済主体が別の利用枠を得ない。

WAN provisioning は暗号化鍵ストア内で PQ 発行者秘密鍵を独立生成し、既存の Ed25519 秘密鍵と共に issuer に渡す。通常の暗号 API は test-support の公開 fixture を使用しない。受領者の匿名資格 scalar は既存の認証付き暗号化鍵ストアで保持する。ライフサイクル監査イベントは `AuditCheckpoint` 用途の両署名で認証し、暗号化 state の版を v2 とする。起動時には監査鎖を検証するため、異なる署名鍵での復元は拒否する。

旧 `QOMMKYB1` state は上書き・削除・自動再発行せず、過去記録の checkpoint と明示的な PQ 再登録が必要であると返す。外部発行者の本番 trust anchor は運用者が独立に登録する。acceptance 用外部 provider process の鍵を本番の本人確認機関の認証証拠とは扱わない。

この変更は既存 OR-DLEQ/Ristretto による匿名メンバーシップ関係、Pedersen 関係、scope-nullifier の量子安全性を証明しない。発行者認証・保存時暗号化・公開 statement の認証と、隠れた属性関係の証明は別の境界である。

専用 remote run `20260905T223820Z-qomm-rust` では lifecycle 1、wire 2、外部 assertion 4、KYB 10、policy contract 24 試験が成功した。全体 run はその後の SDK fixture 型差分で失敗しており、全 P1–P6 完了の証跡ではない。最終受入には現在 source での全 consumer / native gate の再実行が必要。
