# CI/CD GitLab

La configuration [`.gitlab-ci.yml`](../.gitlab-ci.yml) cible **Linux x86_64**,
sans compilation croisée.

## Préparer GitLab et le runner

1. Enregistrer un runner **Linux x86_64 avec exécuteur Docker** pouvant prendre
   les jobs sans tag. Sinon, ajouter les tags du runner aux jobs concernés.
2. Autoriser les images `node:22-bookworm`, `orhunp/git-cliff:2.10.1` et
   `registry.gitlab.com/gitlab-org/cli:v1.76.0`, les accès aux dépôts Debian/Alpine,
   à Rust, à Cargo et à l'API GitLab. Aucun mode privilégié ni socket Docker monté
   n'est nécessaire. Le mot-clé `release:` nécessite un GitLab/Runner récent
   prenant en charge `glab` (GitLab 18.0+).
3. Protéger la branche **`main`** et les tags **`client-v*`**, **`server-v*`**.
   Autoriser le compte de release à pousser sur `main` et à créer ces tags.
   Les règles d'approbation du projet doivent permettre ce commit de version
   automatique ; aucun contournement de protection n'est implémenté.
4. Créer un **jeton d'accès de projet** avec `write_repository`, ou à défaut
   un jeton personnel d'un compte technique avec le même droit. Son rôle doit
   permettre les opérations sur les références protégées : choisir **Maintainer**
   si seuls les Maintainers peuvent pousser sur `main`. Les scopes du jeton ne
   donnent pas plus de droits que son compte ; un rôle Guest ne suffit pas.
5. Dans **Settings → CI/CD → Variables**, enregistrer ce jeton sous
   **`GITLAB_RELEASE_TOKEN`**, avec les options **Masked** et **Protected**,
   et une portée d'environnement `*`. Utiliser le type **Variable**, pas **File**,
   et renseigner uniquement la valeur du jeton. Prévoir son renouvellement avant expiration.
6. Activer le **Package Registry** du projet et conserver les paquets génériques
   `romstore-client` et `romstore-server` : les releases pointent sur ces fichiers,
   et non sur les artifacts temporaires des jobs.

La préparation utilise ce jeton uniquement pour Git. Contrairement à un push
authentifié avec `CI_JOB_TOKEN`, son push déclenche bien les pipelines de branche
et de tag. La publication utilise le **`CI_JOB_TOKEN` temporaire** de la pipeline
de tag pour le registre générique et l'API Releases ; aucun droit `api` n'est
nécessaire sur le jeton de push.

Le script fixe explicitement l'utilisateur `oauth2` dans les URL de lecture et
de push, fournit le jeton uniquement via `GIT_ASKPASS` et neutralise les helpers
de credentials et en-têtes HTTP hérités pour cette URL. Cela évite notamment
qu'une réécriture d'URL du runner remplace le jeton de release par `CI_JOB_TOKEN`.
Le secret n'est jamais enregistré dans l'URL ou la configuration Git.

Un push en **dry-run** vérifie l'accès au service Git en écriture avant les
compilations et la création du commit de version. Ce contrôle ne remplace pas
les règles des branches/tags protégés ni les hooks serveur, vérifiés lors du
push réel. Si le refus 403 persiste, vérifier le jeton, son rôle et ces règles,
sans afficher de configuration Git brute susceptible de contenir des secrets.

Un changement de rôle du même jeton peut être testé en relançant le job. En
revanche, une modification de ce script nécessite de pousser le correctif puis
d'utiliser **la nouvelle pipeline de `main`** : relancer un ancien job exécute
toujours les sources de son ancien commit.

L'image de compilation est Debian 12 Bookworm, avec Node 22, **Rust 1.90.0**,
Clippy et rustfmt. SDL2 et les polices sont installées uniquement pour le client
ou les vérifications communes. L'installation s'effectue en root dans le conteneur,
pas sur l'hôte. Le cache conserve le registre et les dépôts Cargo, pas les `target`.
Configurer le cache distribué du runner si les jobs tournent sur plusieurs machines.

La CI et les releases **ne nécessitent pas Python** :

| Fonction | Outil |
|---|---|
| Version, lock, commit et tag | `cargo-release` **1.0.0**, compatible Rust 1.90 |
| Notes de release | `git-cliff` **2.10.1**, configuré dans `cliff.toml` |
| Sélection des composants et garde-fous Git | Bash et Git |
| Archive et empreinte | `tar`, `od` et `sha256sum` |
| Lecture des métadonnées Cargo | `cargo metadata` et `jq` |
| Envoi au registre générique | `curl` avec `CI_JOB_TOKEN` |
| Création de la release | Mot-clé GitLab `release:` et `glab` |

Les fichiers `client/release.toml` et `server/release.toml` désactivent la
publication vers crates.io et le push de `cargo-release`, définissent les tags
indépendants et appellent le hook de vérification du composant. Le push CI reste
explicite et atomique. Ne pas remplacer les versions épinglées sans vérifier
leur version Rust minimale ; les dernières versions de `cargo-release` peuvent
nécessiter une version de Rust plus récente que celle du projet.

## Pipeline de branche / merge request

```text
check → test → build → release (main uniquement)
```

- **check** : formatage Rust, Clippy avec avertissements interdits et `make test-ci`
  pour les scripts Bash de CI/release ;
- **test** : tests JavaScript de l'interface d'administration et tests unitaires
  et d'intégration Rust des deux composants ;
- **build** : compilation release, tests E2E avec le véritable client, test SDL
  sans écran et test de densité du texte ;
- **release** : ouvre la pipeline enfant contenant les boutons de release.

Une branche ayant une merge request ouverte utilise la pipeline de MR plutôt que
de lancer deux pipelines à chaque push. Les pipelines de MR n'ont jamais de jobs
de préparation ou de publication de release.

## Releases manuelles depuis `main`

Après réussite des vérifications, ouvrir le job de déclenchement **release**, puis
sa **pipeline enfant** dans le graphe GitLab. Les jobs manuels sont dans son stage
`release` : GitLab ne permet pas d'injecter des jobs dynamiques dans le parent.

| Changements restant à publier | Boutons proposés |
|---|---|
| Client uniquement | `client:fix`, `client:minor`, `client:major` |
| Serveur uniquement | `server:fix`, `server:minor`, `server:major` |
| Les deux composants / fichiers partagés | Les six jobs |
| Aucun changement | Un job informatif, sans bouton de release |

Les changements sont comparés **au dernier tag accessible du composant**, pas au
dernier push : une modification client reste donc à publier même si les derniers
commits ne touchent que le serveur. Sans tag, le composant est proposé pour sa
première release. Les tags invalides ou non ancêtres de `main` sont ignorés.
La comparaison porte sur la différence nette des fichiers ; une modification
entièrement annulée depuis le tag ne nécessite pas de release.

Les chemins sous `client/` concernent le client ; ceux sous `server/` concernent
le serveur. **Tout autre chemin est partagé**, y compris documentation, scripts,
tests communs, Makefile et CI. Chaque composant utilise son propre point de
comparaison : si le client a déjà publié un changement partagé, seul le serveur
reste proposé pour ce changement.

Les versions sont indépendantes et lues depuis leurs `Cargo.toml` respectifs :

| Type | Depuis `1.2.3` |
|---|---|
| `fix` | `1.2.4` |
| `minor` | `1.3.0` |
| `major` | `2.0.0` |

Après confirmation, le job :

1. Exige une pipeline enfant de la branche `main` protégée, le commit exact de la
   pipeline parente et un `main` distant n'ayant pas avancé.
2. Récupère les tags et vérifie que le composant a encore des changements à publier.
3. Appelle la cible Make du composant, qui délègue à `cargo-release` l'incrément
   de `package.version` et la mise à jour de `Cargo.lock`, sans mise à jour
   volontaire des dépendances. Son hook relance formatage, Clippy et tests Rust.
4. Crée un commit et un tag annoté `client-vX.Y.Z` ou `server-vX.Y.Z`.
5. Revérifie `main`, puis pousse le commit et le tag **atomiquement**, sans force.
   Si `main` avance entre-temps, le push est rejeté sans publier le tag.

Les préparations client et serveur partagent un verrou `resource_group` pour
éviter les courses. Après une première release, **utiliser la nouvelle pipeline
de `main`** pour publier l'autre composant : les boutons de l'ancienne pipeline
restent visibles mais refusent de publier depuis un commit obsolète. La liste
des boutons est un instantané au moment de la génération, pas une vue actualisée.

Les jobs manuels sont optionnels et ne bloquent pas le statut vert du parent.
Un échec de préparation doit être consulté dans le job enfant : il ne rend pas
la pipeline parente rouge. Ne pas lancer plusieurs types de release pour le
même composant : choisir un seul incrément. Aucun push n'est effectué si les
contrôles échouent. Un échec du push laisse seulement les changements locaux
dans l'environnement éphémère du job, sans annulation ni force-push.

## Pipeline de tag : publication automatique

```text
check → test → build → publish
```

Un tag `client-vX.Y.Z` vérifie, teste et compile **le client uniquement**, avec
les tests SDL. Un tag `server-vX.Y.Z` fait de même **pour le serveur uniquement**,
avec les tests WebUI. Les E2E communs ont déjà été exécutés sur `main`.
Les autres tags, dont les anciens tags communs `vX.Y.Z`, ne créent pas de pipeline.

Le tag doit correspondre exactement à la version du manifeste ciblé. Après
validation du binaire ELF64 x86_64, la publication :

1. Prépare l'archive et `SHA256SUMS`.
2. Génère `CHANGELOG.md` avec `git-cliff`, dans le job parallèle `notes`, depuis
   les commits entre ce tag et le précédent tag du même composant, avec liens
   de comparaison et de commits GitLab.
3. Envoie les trois fichiers au registre de paquets génériques GitLab.
4. Le mot-clé GitLab `release:` crée la release et ses liens de téléchargement
   **uniquement après la réussite de tous les envois**. Une release existante
   n'est jamais modifiée.

Un tag suffixé `-rc.1` reste identifiable par son nom mais crée une release
GitLab ordinaire.
Les préreleases se préparent manuellement : les boutons d'incrément exigent une
version actuelle stable `X.Y.Z`.

L'historique complet est requis (`GIT_DEPTH: '0'`). Pour la première release,
les notes présentent tout l'historique du composant jusqu'au tag. Les messages
ne sont pas filtrés par répertoire afin d'inclure les changements partagés.
`SHA256SUMS` vérifie l'archive ; le changelog est un fichier joint séparé.

Exemple de fichiers d'une release client :

```text
romstore-client-0.1.0-linux-x86_64.tar.gz
SHA256SUMS
CHANGELOG.md
```

Les fichiers locaux sont isolés dans `dist/<tag>/`. Les artifacts de build
expirent après sept jours ; les fichiers du Package Registry restent disponibles
tant qu'aucune règle de nettoyage ne les supprime.

En cas d'échec de publication, **réparer et relancer la pipeline de ce tag** avant
de préparer une nouvelle version : la détection utilise les tags, même si leur
publication a échoué. Si des fichiers ont été envoyés partiellement, supprimer
le paquet générique incomplet avant de relancer. Ne jamais supprimer une release
publiée ou remplacer silencieusement ses fichiers pour corriger ses binaires ;
préparer une nouvelle version. Les publications d'un même tag sont sérialisées.

## Préparer une release localement

Depuis un dépôt propre, avec Bash, Git configuré, `jq` et l'environnement Rust
du composant, installer une fois les outils épinglés :

```bash
# Debian/Ubuntu : prérequis de compilation de cargo-release
sudo apt-get install build-essential pkg-config libssl-dev jq
make release-tools
```

`cargo-release` est une **extension**, pas une commande fournie avec Cargo.
La cible installe aussi `git-cliff`, utile pour générer les notes localement.
`CARGO_RELEASE_VERSION` et `GIT_CLIFF_VERSION` permettent de surcharger les versions
d'installation ; la CI utilise les versions épinglées dans sa configuration.

Les commandes de préparation restent identiques :

```bash
make release-client TYPE=fix
make release-server TYPE=minor
```

`fix` est traduit en `patch`, le nom du niveau correctif dans `cargo-release`.
`CARGO` peut être surchargé, par exemple `make release-client TYPE=fix CARGO='cargo +stable'`.

Elles refusent un dépôt sale, un HEAD détaché et une version actuelle non stable,
préparent uniquement le composant choisi, exécutent les contrôles et créent
le commit et le tag **sans push ni publication vers crates.io**.
Si `user.signingkey` est configuré, le commit
et le tag sont signés ; un échec de signature n'entraîne aucun repli non signé.
Vérifier le résultat puis pousser la branche et le tag ensemble :

```bash
git push --atomic origin HEAD:main refs/tags/client-v0.1.1
```

Adapter le tag. Pour une prérelease, modifier et committer manuellement la version
et le lock, puis créer le tag correspondant. Un tag poussé manuellement déclenche
les mêmes vérifications et la même publication que depuis les boutons GitLab.
Les protections GitLab restent applicables aux opérations locales.
En cas d'échec, inspecter `git status`, `git log` et les tags avant de relancer :
aucune annulation automatique de modifications, de commit ou de tag n'est effectuée.

## Utiliser les archives

Télécharger l'archive et `SHA256SUMS` de **la même release** :

```bash
sha256sum -c SHA256SUMS
tar -xzf romstore-client-0.1.0-linux-x86_64.tar.gz
```

Ce sont des binaires **dynamiques Debian 12**, nécessitant **glibc 2.36+**.
Le client requiert aussi **SDL2 2.26+** et une police TrueType :

```bash
sudo apt-get install libsdl2-2.0-0 fonts-dejavu-core
```

Copier `config.example.yaml` vers un fichier personnel et adapter ses chemins
avant de lancer `romstore-client --config /chemin/config.yaml`. Le serveur utilise
les options de `docs/server.md` et ne nécessite pas SDL2.
L'archive contient le binaire et sa documentation, jamais de configuration privée,
ROM, base SQLite ni cache. Ce ne sont **pas des installateurs Batocera** : aucune
entrée Ports ni configuration n'est créée automatiquement. Vérifier séparément
la compatibilité de la version de Batocera (glibc/SDL2 et matériel).

## Vérification locale des outils CI

```bash
make test-ci
bash scripts/ci/generate-releases.sh dist/release-jobs.yml
make client
bash scripts/ci/package.sh client-v0.1.0
bash scripts/ci/changelog.sh client-v0.1.0
```

Adapter le tag à la version du manifeste, ou utiliser `make server` puis
`package.sh server-vX.Y.Z`. Pour `changelog.sh`, le tag doit exister localement ;
définir `CI_PROJECT_URL` pour inclure les liens GitLab dans les notes locales.
Ces commandes n'effectuent aucun accès à l'API GitLab.

`make test-ci` travaille dans des dépôts temporaires : détection des composants,
tags indépendants, hooks, options Make, packaging, sommes de contrôle et erreurs
d'envoi simulées. Si les outils sont installés avec `make release-tools`, il teste
aussi le vrai `git-cliff` et une release avec le vrai `cargo-release`, jusqu'au
push atomique **vers un dépôt Git local isolé**, jamais vers le dépôt du projet.

La variable Make `PYTHON` ne sert plus qu'à `make fixtures` (`test/generate.py`),
qui reste un générateur de données de test indépendant de la CI/release.
