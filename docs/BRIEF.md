# Drifterr — Brief technique & stratégique complet

> **Document d'onboarding pour le développeur full-stack.**
> Objectif : que tu puisses lire ce seul document et comprendre *quoi* on construit, *pourquoi*, *comment*, et *par où commencer*. Tout est ici : la vision, l'architecture, le modèle de données, les contrats entre composants, la stack, et un plan de build avec critères d'acceptation.

---

# PARTIE I — STRATÉGIE (le quoi & le pourquoi)

## 1. Le produit en une phrase
**Drifterr est un copilote local qui surveille si ta session de chat IA en cours dérive de ce que tu lui as demandé — et te prévient avant que tu perdes une heure, avec un reset en un clic.**

Tagline : *« Ton modèle n'a pas changé. Ta conversation, si. Drifterr te prévient avant le mur. »*

## 2. Le problème
Tu démarres une session avec Claude, ChatGPT, Cursor ou un agent. Au début c'est brillant : le modèle comprend le but, respecte tes contraintes, construit sur vos décisions. Puis ça glisse — une contrainte posée il y a 20 min disparaît, une idée rejetée revient, les réponses tournent en rond. Tu te bats dix tours sans comprendre pourquoi le modèle est devenu « bête ».

**Ce n'est pas le modèle qui a changé.** C'est un phénomène mécanique et documenté : le **context rot**. Plus le contexte se remplit, plus l'attention se dilue, et moins tes instructions initiales pèsent sur chaque réponse.

**L'insight clé qui rend Drifterr solide** : ce n'est pas une perception floue. C'est une déviation **mesurable** par rapport à une **vérité-terrain qui appartient à l'utilisateur** — le but et les contraintes *qu'il a lui-même posés*. On ne prétend jamais détecter « le modèle a empiré » (perceptuel, invérifiable). On détecte « cette session a divergé de ce que tu as posé » (mesurable, vérifiable).

## 3. Pourquoi personne ne le résout (la brèche)
| Catégorie existante | Ce qu'ils font | Ce qui manque |
|---|---|---|
| Observabilité (LangSmith, Galileo, Confident AI) | Dashboards serveur, apps en prod, post-hoc, équipes | Pas un compagnon **live** dans *tes propres* chats |
| Gestion de contexte native (`/compact`, règles Cursor, status line) | Compactage / fichiers de règles | **Mono-outil** ; mesure le **remplissage**, pas la **divergence d'intention** |
| Conseils manuels (« reset quand ça dérive ») | Bonne pratique | **Réactif** : à toi de remarquer — toujours trop tard |

**Personne ne combine** : transversal × live & proactif × relatif à l'intention × saturation exacte × réancrage automatique × apprentissage d'ordres permanents. C'est notre territoire.

## 4. Le moat (défensibilité)
Le vrai moat n'est pas un algo (copiable) ni un dataset. C'est **personnel et composé** :

> À force d'observer les sessions, Drifterr apprend **les contraintes récurrentes de l'utilisateur** — celles qu'il répète de projet en projet — et construit automatiquement sa couche d'« ordres permanents » (son `CLAUDE.md` / `.cursorrules` vivant).

Chaque correction faite trois fois devient une règle persistante. Plus l'utilisateur s'en sert, plus Drifterr le connaît, plus il est dur à quitter.

## 5. Go-to-market
**Wedge = développeurs** qui contrôlent leurs appels API (Cline, agents custom, OpenRouter) et utilisateurs de Claude Code. Expansion ensuite vers le grand public via extension navigateur (claude.ai / ChatGPT / Gemini).

## 6. Business model
- **Gratuit** : 1 projet actif, signal de saturation + dégradation, reset manuel.
- **Pro (~7 €/mois, 59 €/an)** : projets illimités, tous les signaux, snapshots auto, journal de décisions, ordres permanents auto-appris, historique.
- **Team / Enterprise** : règles partagées, gouvernance, rapports d'alignement.

---

# PARTIE II — GLOSSAIRE
- **Context rot** : dégradation mécanique de la qualité d'un LLM à mesure que sa fenêtre de contexte se remplit.
- **Baseline / Empreinte d'intention** : le triplet `{ but, contraintes, décisions }` extrait au début d'une session. Vérité-terrain contre laquelle on mesure la dérive.
- **Signal** : une dimension mesurable de dérive (5 au total). Jamais fondus en un score unique opaque.
- **Signal dur** : déclenchable de façon quasi-déterministe (contraintes, saturation). Déclenche les alertes rouges.
- **Signal mou** : flou par nature (but, dégradation). Support/contexte, jamais seul déclencheur.
- **Canal / Adaptateur** : la source d'où vient la conversation (proxy, fichiers, navigateur). Produit toujours le **format normalisé**.
- **Réancrage** : l'intervention qui remet la session sur les rails.
- **Ordres permanents** : règles persistantes apprises des corrections récurrentes (le moat).

---

# PARTIE III — ARCHITECTURE

Le principe non négociable : le moteur est **channel-agnostic**. Tout adaptateur produit la même structure de données. On écrit le moteur une fois ; ajouter un canal = écrire un adaptateur. Tout est **local-first**.

## Structure de repo (monorepo)
```
drifterr/
├── apps/
│   ├── desktop/                 # App Tauri 2 (tray menubar + UI panneau)
│   └── extension/               # Extension navigateur (TS, MV3)
├── crates/
│   ├── proxy/                   # Proxy API local (axum + hyper, SSE)
│   ├── engine/                  # Moteur : signaux, state machine
│   ├── adapters/                # Watcher fichiers + normalisation
│   ├── intervention/            # Génération snapshot + réancrage
│   ├── store/                   # Couche SQLite (rusqlite)
│   └── tokenizer/               # Comptage de tokens par fournisseur
├── fixtures/                    # Transcripts de test (validation moteur)
└── README.md
```

---

# PARTIE IV — SPÉCIFICATION TECHNIQUE

## 1. Le format normalisé (le contrat central)
Tout adaptateur produit ceci, et rien d'autre :
```ts
type Turn = { index: number; role: "user"|"assistant"|"tool"; content: string; tokens: number; timestamp: number; };
type ContextState = { windowSize: number; usedTokens: number; exact: boolean; toolCallCount: number; };
type Conversation = { sessionId: string; model: string; turns: Turn[]; context: ContextState; source: "proxy"|"file"|"browser"; };
```
> Le champ `exact` dit à l'UI si le « % de contexte » est une vérité ou une estimation. **On ne ment jamais sur la précision.**

## 2. Les canaux (par ordre d'implémentation)
1. **Proxy API local** (premier) — seul canal donnant la **saturation exacte**. Passthrough SSE + tee. ⚠️ Ne jamais bufferiser la réponse avant de la renvoyer.
2. **Watcher de fichiers** — suit les sessions Claude Code sur disque (`notify`). `exact = false`.
3. **Extension navigateur** — content script MV3 lisant le DOM (jamais d'image). `exact = false`.
- **OCR / capture d'écran : exclu du produit.**

## 3. Le moteur de détection
### 3.1 Baseline (empreinte d'intention) — `{ goal, constraints, decisions }`.
### 3.2 Les 5 signaux (séparés, jamais fondus)
| # | Signal | Méthode | Type |
|---|--------|---------|------|
| 1 | Respect des contraintes | déterministe (regex/parse) → 0 faux positif ; judge pour le flou | dur |
| 4 | Saturation de contexte | `usedTokens / windowSize` + fill rate + volume outils | dur |
| 2 | Alignement au but | embedding goal vs tours (ONNX local), tendance cosinus | mou |
| 3 | Cohérence des décisions | retrieval + judge (idée rejetée réintroduite ?) | mi-dur |
| 5 | Symptômes de dégradation | stats texte (verbosité, boucle, hedging) | mou |

**Règle d'or** : chaque signal garde son **état** et sa **preuve**. L'UI nomme le signal — jamais un « -14 % » fourre-tout.

### 3.4 State machine
- **Vert** aligné · **Ambre** saturation montante ou 1 signal mou qui glisse · **Rouge** signal dur déclenché ou plusieurs mous convergents.
- **Anti-flicker** : hystérésis (N confirmations / debounce). Les signaux durs priment.

## 4. L'intervention (« Réancrer »)
Snapshot de reset (markdown collable), préambule de réancrage, auto-injection proxy (opt-in).

## 5. Apprentissage des ordres permanents (le moat)
Corrections récurrentes trackées (embeddings pour dédup) → ≥ 3 occurrences → promotion en règle persistante.

## 6. Stack
Tauri 2 · proxy Rust (axum + hyper, SSE) · moteur Rust · embeddings ONNX local (`fastembed-rs`) · judge pluggable (Haiku cloud ou LLM local) · tokenizer (`tiktoken-rs` / count-tokens Anthropic) · SQLite (`rusqlite`/`sqlx`) · watcher `notify` · extension TS MV3.

## 7. Modèle de données — voir `crates/store/src/schema.sql`.

## 8. Confidentialité
Local-first ; deux modes de judge (cloud minimal vs local total) ; télémétrie opt-in ; « Drifterr ne filme pas ton écran ».

## 9. Coût
Saturation + dégradation + contraintes déterministes = 0 modèle. Embeddings locaux = 0. Judge seulement sur les deltas → centimes/session ou 0 en local.

---

# PARTIE V — PLAN DE BUILD
- **M0** — Fondations : monorepo, schéma SQLite, format normalisé, tokenizer. *Acceptation* : charger une `Conversation` depuis une fixture JSON et la persister/relire.
- **M1** — Moteur, signaux durs : baseline, Signal 1 (contraintes déterministes), Signal 4 (saturation). *Acceptation* : sur des transcripts annotés, le moteur signale correctement violations et seuils, sans canal réel. **Go/no-go du produit.**
- **M2** — Canal proxy + UI minimale (passthrough SSE sans dégradation, widget rouge sur violation, saturation exacte).
- **M3** — Signaux mous (2,3,5) en support + intervention (snapshot collable restaure l'alignement).
- **M4** — Canaux fichiers + navigateur (les trois alimentent le même moteur, sans branche spécifique).
- **M5** — Ordres permanents + auto-réancrage proxy opt-in.

---

# PARTIE VI — LES VRAIS POINTS DURS
1. **Passthrough SSE du proxy** — bufferiser casse le streaming. Tee strict, testé sur les deux schémas et des outils réels.
2. **Faux positifs des signaux mous** — seuls les signaux durs déclenchent le rouge.
3. **Surfaces fermées** (Cursor, Copilot chat) — ne pas bâtir le MVP dessus ; proxy + fichiers les contournent.
4. **Commoditisation** — défense : cross-tool + ordres permanents personnels.

---

# PARTIE VII — DÉCISIONS OUVERTES
- UI panneau : React ou Svelte ?
- Judge cloud par défaut : quel modèle ? mode local dès le MVP ou fast-follow ?
- Modèle d'embedding local pour `fastembed-rs` ?
- Premier outil cible du proxy : Cline, OpenRouter, ou transcripts Claude Code ?
- Format des ordres permanents : écrire dans `CLAUDE.md` / `.cursorrules`, ou store interne + export ?
