# Serveur Rust : compilation, déploiement et migration

Le serveur est désormais un **binaire Rust autonome**, basé sur Axum/Tokio et
Rusqlite. Il ne nécessite plus Python, FastAPI, Uvicorn, pip ni environnement `.venv`.
SQLite est compilé avec le binaire ; aucune bibliothèque SQLite système n'est requise.
Le serveur cible Linux/Unix. Aucun composant SDL n'est nécessaire côté serveur.

## Compiler et lancer

Depuis la racine du dépôt, avec Rust 1.90+ et un compilateur C :

```bash
cargo build --release --locked --manifest-path server/Cargo.toml
./server/target/release/romstore-server serve \
  --library /srv/roms \
  --database /srv/romstore/catalog.sqlite3 \
  --system-icon /etc/romstore/system-icons \
  --host 0.0.0.0 --port 8000
```

Le compte qui lance le serveur doit pouvoir lire les ROMs/XML/icônes et écrire dans
le dossier de la base. Il n'a pas besoin de droits d'écriture sur la bibliothèque.
Les chemins relatifs dépendent toujours du répertoire de lancement.

Un binaire précompilé doit correspondre à l'architecture et à la libc du serveur.
Le build conteneur fourni utilise Debian Bookworm (glibc 2.36) ; il n'est pas destiné
à Alpine/musl ni aux anciens NAS avec une glibc plus ancienne. Compiler directement
sur la cible ou utiliser le conteneur évite de copier un binaire Linux incompatible.

| Option | Défaut / rôle |
|---|---|
| `serve` ou `index` | `serve` par défaut ; `index` réindexe puis affiche son bilan JSON |
| `--library` | Obligatoire : racine des dossiers de plateformes |
| `--database` | `.romstore/catalog.sqlite3`, obligatoirement hors bibliothèque |
| `--system-icon` | `system-icons` à côté de la base ; fichiers `nes.png`, `snes.webp`… |
| `--host` | `127.0.0.1` ; utiliser une adresse LAN ou `0.0.0.0` pour les clients distants |
| `--port` | `8000`, configurable comme avant (par exemple `6789`) |
| `--log-level` | `info`, ou `debug`, `warning`, `error`, `critical` (alias `error`) |

`RUST_LOG`, s'il est défini, prend priorité et permet de filtrer par module.
Les logs horodatés vont sur stderr ; stdout reste du JSON pour la commande `index`.
Le suivi par plateforme, par lot de 100 jeux et par tranche d'environ 5 secondes
reste disponible, ainsi que l'avancement des SHA-256 longs.

L’accès HTTP démarre sans attendre l’indexation initiale : celle-ci utilise la même
tâche de fond que le rechargement via `POST /api/v1/catalog/reload`. Les lecteurs
continuent à voir l’ancien catalogue jusqu’à la publication atomique ; au premier
lancement, le catalogue est vide pendant l’analyse. Une erreur d’analyse est signalée
sans arrêter le serveur et sans effacer le catalogue précédent. La commande `index`
reste synchrone et n’ouvre pas d’accès HTTP. Un verrou fichier
`catalog.index.lock` à côté de la base protège aussi contre les indexations simultanées
entre plusieurs processus Rust utilisant cette même base.

## Interface web et suivi côté client

Ouvrir `http://adresse:port/` pour accéder à la gestion intégrée, sans dépendance
externe : état de l’indexation, bouton de relance et compteurs de ROMs par système
(total et installables). Les compteurs décrivent le dernier catalogue publié, pas
une estimation du scan en cours ; un jeu multidisque représente une entrée.
La page s’actualise toutes les 3 secondes et désactive la relance pendant l’analyse.
Les vérifications périodiques conservent l’état visuel du bouton : il n’est désactivé
que pendant une demande de relance, une analyse ou une erreur de connexion, sans
clignotement à chaque requête. Un clic pendant une vérification est mis en attente.

Le client SDL surveille aussi `GET /api/v1/catalog/reload` toutes les 3 secondes,
y compris pour une analyse lancée au démarrage ou depuis la page web. Il affiche
l’indexation en cours sans bloquer la navigation, masque le raccourci de relance et
rafraîchit ses données à la fin de l’analyse. Toutes les demandes dans le processus
serveur partagent le même état et ne lancent pas plusieurs analyses simultanées.
`GET /api/v1/catalog/summary` expose les compteurs par système sans lire les ROMs.
La section **Anomalies du catalogue** liste les avertissements enregistrés et les
jeux non installables, avec filtre par système et pagination. Elle identifie le jeu,
son chemin et la raison ; les nouvelles entrées XML ignorées mentionnent aussi leur
numéro de ligne. L’API correspondante est `GET /api/v1/catalog/issues`, avec paramètres
`system` facultatif, `offset` et `limit` (1–200, défaut 50). Les anciens diagnostics
génériques restent lisibles mais ne deviennent précis qu’après une nouvelle analyse.

**La page de gestion n’est pas authentifiée**, comme l’API : réseau de confiance
uniquement, ou reverse proxy avec authentification.

Les routes `/api/v1/…`, `/openapi.json` et `/docs` sont inchangées. Le schéma OpenAPI
est produit depuis les modèles et routes Rust. L'interface Swagger charge ses assets
depuis un CDN ; le schéma JSON reste disponible sans accès Internet.

## Migrer l'installation Python existante

1. **Arrêter l'ancien serveur**. Ne pas exécuter les deux versions simultanément sur
   la même base.
2. Sauvegarder le dossier de données SQLite, base et éventuels fichiers `-wal`/`-shm`
   compris. Ne pas supprimer la bibliothèque ni son cache d'empreintes.
3. Compiler le binaire Rust, puis reprendre **exactement les mêmes chemins et options**
   de lancement, en remplaçant seulement l'ancien exécutable `.venv/bin/romstore-server`
   par `server/target/release/romstore-server` (ou son chemin installé).
4. Si un service systemd/superviseur est utilisé, adapter son `ExecStart` et conserver
   son répertoire de travail ou passer des chemins absolus, puis redémarrer ce service.
5. Contrôler les logs et `GET /api/v1/health`. Les clients existants restent compatibles
   et leur `server_url` ne change pas si l'adresse et le port sont conservés.

Les tables SQLite et les signatures SHA-256 sont conservées. Les identifiants des
jeux/fichiers et les révisions restent identiques pour un contenu inchangé, y compris
les noms accentués, guillemets et caractères hors BMP. Le format historique de calcul
des révisions est explicitement conservé ; ce n'est pas le format compact JSON du
nouveau sérialiseur qui détermine ces empreintes.

Lors de la première ouverture, le serveur reconstruit uniquement l'index de recherche
Unicode SQLite et marque son format avec `PRAGMA user_version=1`. Les données et le
cache ne sont pas effacés par cette migration. Ensuite, le scan normal réutilise les
empreintes dont le chemin résolu et la signature du fichier n'ont pas changé.
Un déplacement des ROMs, un changement de montage ou une migration vers un conteneur
avec des chemins différents peut donc entraîner de nouveaux calculs, sans perte de données.

L'ancien paquet Python et ses tests ont été retirés. Un `.venv` déjà présent peut être
laissé en place : il n'est simplement plus utilisé par le serveur. Les scripts Python
de packaging et d'installation du client Batocera restent indépendants et ne sont pas
une dépendance du serveur ni du client en cours d'exécution.

## Conteneur Docker / Podman

Depuis la racine du dépôt :

```bash
docker build -t romstore-server -f server/Dockerfile server
```

Exemple Linux, après préparation des dossiers et permissions de données/icônes :

```bash
docker run --rm --name romstore-server \
  --user "$(id -u):$(id -g)" \
  -p 8000:8000 \
  -v /srv/roms:/library:ro \
  -v /srv/romstore:/data \
  -v /etc/romstore/system-icons:/icons:ro \
  romstore-server
```

La configuration par défaut de l'image utilise `/library`, `/data/catalog.sqlite3`
et `/icons`. Le volume `/data` doit être persistant et accessible en écriture au compte
choisi. Le système de fichiers de la bibliothèque et des icônes est monté **en lecture
seule**. Le runtime de l'image s'exécute sans root par défaut (UID/GID 10001) ; l'option
`--user` de l'exemple adapte cela aux permissions du compte hôte. Adapter les labels
de volumes si SELinux est actif. Avec Podman rootless, ajouter `--userns=keep-id`
pour conserver l'identité hôte utilisée par `--user` sur les volumes liés.

## Sécurité et limites

- Pas d'authentification ni de TLS natif : réserver l'accès à un réseau de confiance
  ou ajouter un reverse proxy avec authentification/TLS avant toute exposition.
- Le serveur ne fournit aucune route de suppression ou écriture dans les ROMs.
- Les chemins, symlinks, DTD/entités et changements de fichiers sont toujours contrôlés.
- Le XML source est borné à 16 Mio ; UTF-8, UTF-16 et les encodages déclarés pris en
  charge (dont ISO-8859-1) sont décodés avant analyse. Les XML du client installé restent UTF-8.
- Les fichiers sont envoyés en flux borné, avec ETag/If-Match et Content-Length ; pas
  encore de reprise HTTP Range.
- Les icônes restent dynamiques, optionnelles, limitées à 8 Mio et hors des manifestes.
- Le premier scan doit toujours lire les ROMs et médias non présents dans le cache :
  le passage à Rust n'élimine pas ce coût disque/réseau ni la nécessité des SHA-256.

## Tests

```bash
cargo test --locked --manifest-path server/Cargo.toml
cargo clippy --locked --manifest-path server/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path server/Cargo.toml -- --check
node --test server/tests/admin_ui.test.mjs
cargo build --locked --manifest-path client/Cargo.toml
ROMSTORE_CLIENT="$PWD/client/target/debug/romstore-client" \
  cargo test --locked --manifest-path server/Cargo.toml --test e2e -- --ignored
```

Ces tests sont écrits en Rust et créent leurs propres ROMs synthétiques dans des
dossiers temporaires : ni Python ni une bibliothèque utilisateur ne sont nécessaires
pour tester le serveur. Les tests de bout en bout utilisent le vrai client, y compris
téléchargement des médias, noms échappés, réinstallation, conflits et récupération.
