# CI/CD GitLab

La configuration [`.gitlab-ci.yml`](../.gitlab-ci.yml) cible **Linux x86_64**,
sans compilation croisée.

## Préparer GitLab et le runner

1. Enregistrer un runner **Linux x86_64 avec exécuteur Docker**, tagué `self`.
   Le job `build-env` utilise Buildah (`vfs`, isolation `chroot`) : configurer
   les permissions et profils de sécurité du runner pour autoriser Buildah.
   Aucun socket Docker ni service Docker-in-Docker n'est utilisé par les jobs.
2. Activer le **Container Registry** du projet et autoriser ses images, ainsi que
   `quay.io/buildah/stable`, `rust:1.100-bookworm`,
   `node:22-bookworm-slim`, `orhunp/git-cliff:2.10.1` et
   `registry.gitlab.com/gitlab-org/cli:v1.76.0`, les accès aux dépôts Debian/Alpine,
   à Cargo et à l'API GitLab. Le mot-clé `release:` nécessite un GitLab/Runner récent
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
6. Autoriser la conservation des **artifacts** du job `release-files` avec
   `expire_in: never`. Ne pas supprimer ces artifacts : les fichiers des releases
   y sont stockés. Leur accès suit les permissions GitLab du projet et de ses jobs.

**Framagit désactive le Package Registry au niveau de son instance**, ce qui
renvoie un 404 lors d'un envoi vers `/packages/generic/`. La pipeline n'en dépend
donc pas. Voir [la confirmation de l'administrateur Framasoft](https://framacolibri.org/t/framagit-ci-cd-package-registry-and-images-pull-policy/32331/2).

La préparation utilise ce jeton uniquement pour Git. Contrairement à un push
authentifié avec `CI_JOB_TOKEN`, son push permet de déclencher la pipeline de tag.
Le commit automatique de version sur `main` est exclu par `workflow: rules` :
aucune pipeline de branche supplémentaire n'est créée. La publication utilise
le **`CI_JOB_TOKEN` temporaire** de la pipeline
de tag pour l'API Releases ; aucun droit `api` n'est
nécessaire sur le jeton de push.

Le script fixe explicitement l'utilisateur `oauth2` dans les URL de lecture et
de push et utilise un helper Git non interactif qui lit `GITLAB_RELEASE_TOKEN`
depuis l'environnement. Les helpers de credentials et en-têtes HTTP hérités
sont neutralisés pour cette URL. Cela évite notamment
qu'une réécriture d'URL du runner remplace le jeton de release par `CI_JOB_TOKEN`.
La configuration conserve seulement la référence à la variable, jamais la valeur
du secret. Le helper n'enregistre aucun credential et n'utilise aucun fichier
exécutable temporaire. Il fonctionne aussi lorsque le runner impose
`credential.interactive=false`, qui peut empêcher l'utilisation de `GIT_ASKPASS`.

Un push en **dry-run** vérifie l'accès au service Git en écriture avant les
compilations et la création du commit de version. Ce contrôle ne remplace pas
les règles des branches/tags protégés ni les hooks serveur, vérifiés lors du
push réel. Si le refus 403 persiste, vérifier le jeton, son rôle et ces règles,
sans afficher de configuration Git brute susceptible de contenir des secrets.

Un changement de rôle du même jeton peut être testé en relançant le job. En
revanche, une modification de ce script nécessite de pousser le correctif puis
d'utiliser **la nouvelle pipeline de `main`** : relancer un ancien job exécute
toujours les sources de son ancien commit.

L'image de compilation est Debian 12 Bookworm, avec Node 22, **Rust 1.100**,
Clippy, rustfmt, SDL2, les polices, `cargo-release` et `git-cliff`.
Le job `build-env`, dans le stage `prepare`, construit cette image avec Buildah
et la pousse vers **`$CI_REGISTRY_IMAGE/ci-env:latest`**. L'authentification utilise
`CI_REGISTRY_USER` et `CI_REGISTRY_PASSWORD`, sans jeton supplémentaire.
Il ne s'exécute que si `.gitlab/ci/Dockerfile` change dans un push de branche ou
une merge request. Pour une nouvelle branche, la comparaison se fait avec la
branche par défaut, afin de ne pas reconstruire sans modification du Dockerfile.
Les tags et les pipelines manuelles ou planifiées ne reconstruisent pas l'image.
Le contexte de build est limité à `.gitlab/ci`.

Le template `.rust` utilise directement cette image du registry, avec une
politique de téléchargement `always`. Le tag partagé permet aux branches et aux
tags de release de l'utiliser même si leur pipeline n'exécute pas `build-env`.
Une modification du Dockerfile sur une branche ou une MR remplace donc aussi
cette image partagée ; réserver ces modifications aux contributeurs de confiance.
Les constructions sont sérialisées par `resource_group: ci-env`.
L'image doit déjà exister pour une pipeline sans modification du Dockerfile :
au premier déploiement, pousser une modification du Dockerfile ou construire et
pousser cette image une fois manuellement.
Les outils sont installés dans l'image, pas sur l'hôte.
Le cache GitLab conserve le registre et les dépôts Cargo, pas les `target`.
Configurer le cache distribué du runner si les jobs tournent sur plusieurs machines.

La CI et les releases **ne nécessitent pas Python** :

| Fonction | Outil |
|---|---|
| Version, lock, commit et tag | `cargo-release` **1.0.0**, compatible Rust 1.90 |
| Notes de release | `git-cliff` **2.10.1**, configuré dans `cliff.toml` |
| Sélection des composants et garde-fous Git | Bash et Git |
| Archive et empreinte | `tar`, `od` et `sha256sum` |
| Lecture des métadonnées Cargo | `cargo metadata` et `jq` |
| Conservation des fichiers | Artifacts GitLab du job `release-files`, sans expiration |
| Vérification d'une release existante | `curl` avec `CI_JOB_TOKEN` |
| Création de la release | Mot-clé GitLab `release:` et `glab` |

Les fichiers `client/release.toml` et `server/release.toml` désactivent la
publication vers crates.io et le push de `cargo-release`, définissent les tags
indépendants et appellent le hook de vérification du composant. Le push CI reste
explicite et atomique. Ne pas remplacer les versions épinglées sans vérifier
leur version Rust minimale ; les dernières versions de `cargo-release` peuvent
nécessiter une version de Rust plus récente que celle du projet.

## Pipeline de branche / merge request

```text
build-env (si Dockerfile modifié) → check → test → build → release (main uniquement)
```

- **check** : formatage Rust, Clippy avec avertissements interdits et `make test-ci`
  pour les scripts Bash de CI/release ;
- **test** : tests JavaScript de l'interface d'administration et tests unitaires
  et d'intégration Rust des deux composants ;
- **build** : compilation release, tests E2E avec le véritable client, test SDL
  sans écran et test de densité du texte ;
- **release** : job manuel optionnel et paramétrable, uniquement sur `main`.

Une branche ayant une merge request ouverte utilise la pipeline de MR plutôt que
de lancer deux pipelines à chaque push. Les pipelines de MR n'ont jamais de jobs
de préparation ou de publication de release.

## Releases manuelles depuis `main`

Après réussite des vérifications, cliquer sur le **nom du job `release`** pour
ouvrir son formulaire manuel, plutôt que sur le bouton de lancement rapide.
Renseigner les variables avant de lancer et confirmer :

| Variable | Valeurs | Par défaut |
|---|---|---|
| `RELEASE_COMPONENT` | `client`, `server` | `client` |
| `RELEASE_TYPE` | `fix`, `minor`, `major` | `fix` |

Les listes déroulantes sont disponibles dans **New pipeline** ; le formulaire
du job manuel permet de surcharger ces mêmes variables. Ne jamais y saisir de
secret. Un lancement rapide utilise les valeurs de la pipeline.
Il n'y a plus de pipeline enfant, de YAML généré ni de six boutons distincts.
Le job vérifie au lancement que le composant choisi a des changements à publier ;
sinon il refuse de créer une version.

Les changements sont comparés **au dernier tag accessible du composant**, pas au
dernier push : une modification client reste donc à publier même si les derniers
commits ne touchent que le serveur. Sans tag, le composant peut publier sa
première release. Les tags invalides ou non ancêtres de `main` sont ignorés.
La comparaison porte sur la différence nette des fichiers ; une modification
entièrement annulée depuis le tag ne nécessite pas de release.

Les chemins sous `client/` concernent le client ; ceux sous `server/` concernent
le serveur. **Tout autre chemin est partagé**, y compris documentation, scripts,
tests communs, Makefile et CI. Chaque composant utilise son propre point de
comparaison : si le client a déjà publié un changement partagé, seul le serveur
reste à publier pour ce changement.

Les versions sont indépendantes et lues depuis leurs `Cargo.toml` respectifs :

| Type | Depuis `1.2.3` |
|---|---|
| `fix` | `1.2.4` |
| `minor` | `1.3.0` |
| `major` | `2.0.0` |

Après confirmation, le job :

1. Exige une pipeline de la branche `main` protégée, le commit exact de cette
   pipeline et un `main` distant n'ayant pas avancé.
2. Récupère les tags et vérifie que le composant a encore des changements à publier.
3. Utilise le même script que les cibles Make locales : `cargo-release` incrémente
   `package.version` et met à jour `Cargo.lock`, sans mise à jour volontaire des
   dépendances. Son hook relance formatage, Clippy et tests Rust.
4. Crée un commit et un tag annoté `client-vX.Y.Z` ou `server-vX.Y.Z`.
5. Revérifie `main`, puis pousse le commit et le tag **atomiquement**, sans force.
    Si `main` avance entre-temps, le push est rejeté sans publier le tag.
    La règle de workflow ignore sur `main` le message de commit généré par
    `cargo-release` (`chore(client|server): prépare la version X.Y.Z`), mais
    accepte le tag en priorité. Ne pas ajouter `[skip ci]` au commit : cela
    empêcherait aussi la publication du tag.

Les préparations client et serveur partagent un verrou `resource_group` pour
éviter les courses. Après une première release, l'ancienne pipeline devient
obsolète. Pour publier l'autre composant sans nouveau changement de code,
**lancer manuellement une nouvelle pipeline de `main`** avec **New pipeline**,
puis utiliser son job `release`. La règle d'exclusion ne concerne que les pushs,
pas les pipelines lancées manuellement.

Le job manuel est optionnel et ne bloque pas le statut vert de la pipeline
(`allow_failure: true`). Consulter son statut et ses logs en cas d'échec.
Ne pas lancer plusieurs types de release pour le
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
2. Génère `CHANGELOG.md` avec `git-cliff`, dans le job `release-files`, depuis
   les commits entre ce tag et le précédent tag du même composant, avec liens
   de comparaison et de commits GitLab.
3. Le job `release-files`, toujours dans le stage `build` mais dépendant du job
   `build`, rassemble les trois fichiers, vérifie leur présence et les
   sommes de contrôle, puis les conserve comme artifacts **sans expiration**.
4. Le mot-clé GitLab `release:` crée la release et ses liens de téléchargement
   **uniquement après la réussite de `release-files` et du stockage des artifacts**.
   Les liens désignent l'identifiant exact de ce job, jamais le dernier job d'une
   branche. Une release existante n'est jamais modifiée.

Un tag suffixé `-rc.1` reste identifiable par son nom mais crée une release
GitLab ordinaire.
Les préreleases se préparent manuellement : le job d'incrément exige une
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

Les fichiers locaux sont isolés dans `dist/<tag>/`. Les artifacts intermédiaires
de `build` expirent après sept jours, sans affecter la copie conservée
par `release-files`. Cette copie n'expire pas automatiquement, mais reste
**supprimable manuellement** ou par une politique d'administration de l'instance.
Supprimer le job, ses artifacts ou sa pipeline peut casser les téléchargements
de la release. Prévoir des sauvegardes et respecter les quotas de Framagit.

En cas d'échec de publication, **réparer et relancer la pipeline de ce tag** avant
de préparer une nouvelle version : la détection utilise les tags, même si leur
publication a échoué. Si les artifacts sources ont expiré avant une reprise,
relancer les jobs producteurs puis `release-files` et `publish`. Ne jamais supprimer une release
publiée ou remplacer silencieusement ses fichiers pour corriger ses binaires ;
préparer une nouvelle version. Les publications d'un même tag sont sérialisées.

Attention : si la correction nécessite de modifier les scripts ou le YAML, une
ancienne pipeline de tag conserve son ancien code, y compris au retry. Pousser
la correction sur `main`, puis préparer une nouvelle version, est la solution
la plus sûre ; ne pas déplacer automatiquement un tag existant.

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
make client
bash scripts/ci/package.sh client-v0.1.0
bash scripts/ci/changelog.sh client-v0.1.0
```

Adapter le tag à la version du manifeste, ou utiliser `make server` puis
`package.sh server-vX.Y.Z`. Pour `changelog.sh`, le tag doit exister localement ;
définir `CI_PROJECT_URL` pour inclure les liens GitLab dans les notes locales.
Ces commandes n'effectuent aucun accès à l'API GitLab.

`make test-ci` travaille dans des dépôts temporaires : détection des composants,
tags indépendants, hooks, options Make, packaging, sommes de contrôle, liens vers
les artifacts et contrôles de publication avec API simulée. Si les outils sont
installés avec `make release-tools`, il teste
aussi le vrai `git-cliff` et une release avec le vrai `cargo-release`, jusqu'au
push atomique **vers un dépôt Git local isolé**, jamais vers le dépôt du projet.

La variable Make `PYTHON` ne sert plus qu'à `make fixtures` (`test/generate.py`),
qui reste un générateur de données de test indépendant de la CI/release.
