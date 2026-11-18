# ROM Store pour Batocera

Serveur **Rust / Axum / Tokio** et client **Rust / SDL2** pour parcourir une bibliothèque Batocera à la manette et installer les ROMs, leurs médias et leurs métadonnées sur une autre machine. Les deux applications sont des binaires Rust ; Python n'est plus nécessaire au serveur.

Cette version couvre les jeux à **un fichier principal** (ROM, ISO, CHD, archive conservée telle quelle) et les **jeux multidisques organisés par M3U**. Le serveur ne modifie jamais la bibliothèque source. Voir [PLAN.md](PLAN.md) pour le suivi du développement et [docs/architecture.md](docs/architecture.md) pour les garanties et limites.

## Structure de la bibliothèque source

```text
/srv/roms/
  nes/
    gamelist.xml
    jeu.nes
    images/jeu.png
  psx/
    gamelist.xml
    jeu.chd
    images/jeu.png
```

Les chemins du XML sont relatifs au répertoire du système (`./jeu.nes`, `./images/jeu.png`). Les sous-répertoires sont conservés à l'installation. Les ROMs non référencées dans un `gamelist.xml` ne sont pas encore indexées.

### Jeux multidisques (M3U)

Le `gamelist.xml` référence la playlist, pas chaque disque :

```xml
<game><path>./Mon jeu.m3u</path><name>Mon jeu</name></game>
```

Exemple d'organisation **identique sur le serveur et sur le client** :

```text
psx/
  gamelist.xml
  Mon jeu.m3u
  Mon jeu/
    Mon jeu (Disc 1).chd
    Mon jeu (Disc 2).chd
```

Contenu de `Mon jeu.m3u` :

```text
Mon jeu/Mon jeu (Disc 1).chd
Mon jeu/Mon jeu (Disc 2).chd
```

Le serveur inclut la playlist, tous ses disques et les médias dans le manifeste.
Le client télécharge et vérifie chaque fichier, conserve les dossiers et copie le
M3U **à l'identique** ; seule la playlist devient une entrée du XML local. Les
disques ne sont ni extraits ni convertis (CHD, ISO, RVZ, ZIP, 7Z, etc.).

Les extensions `.m3u` et `.m3u8` sont acceptées, sans distinction de casse. La
playlist doit être UTF-8 (BOM accepté), avec un chemin par ligne ; lignes vides,
commentaires `#` et fins de ligne Windows sont acceptés. Les chemins sont relatifs
au **dossier du M3U**, y compris `../` si la cible reste dans la plateforme. Les
références répétées conservent leur place dans le M3U mais ne sont téléchargées
qu'une fois. Limites : 1 Mio et 240 références de disques par playlist.

Un disque absent, un chemin absolu/URL ou sortant de la plateforme, une playlist
vide/invalide, une playlist imbriquée ou un disque à dépendances (`.cue`, `.gdi`,
`.ccd`, `.mds`) rend le jeu visible mais **non installable**, avec un diagnostic.
Après ajout ou modification, ouvrir **Start / Échap → Recharger le catalogue serveur**.

### Images des systèmes

Les icônes sont stockées **dans un dossier distinct sur le serveur**, désigné par `--system-icon`. Elles ne sont plus recherchées dans les dossiers des ROMs. Par exemple, à côté des fichiers de configuration du serveur :

```text
/etc/romstore/
  system-icons/
    nes.png
    snes.png
    psx.webp
```

```bash
./server/target/release/romstore-server serve \
  --library /srv/roms \
  --system-icon /etc/romstore/system-icons
```

Le nom du fichier correspond exactement à l'**identifiant de plateforme** (le nom du dossier de ROMs). Les extensions reconnues, par ordre de priorité, sont `.png`, `.jpg`, `.jpeg`, `.webp`, `.pnm` et `.ppm` (minuscules). Par exemple, `nes.png` représente la plateforme `nes`. Aucun champ supplémentaire n'est nécessaire dans `gamelist.xml` et le client ne nécessite aucun changement de configuration.

Sans `--system-icon`, le dossier par défaut est `system-icons` à côté de la base du serveur : **`.romstore/system-icons`** avec la base par défaut, ou `/etc/romstore/system-icons` si la base est `/etc/romstore/catalog.sqlite3`. Les chemins relatifs de la CLI sont résolus depuis le répertoire de lancement. Le dossier peut être créé après le démarrage ; le serveur n'écrit pas dedans.

Pour migrer l'ancien emplacement, copier par exemple `/srv/roms/nes/system.png` vers `/etc/romstore/system-icons/nes.png`. Aucun fichier existant n'est déplacé ou supprimé automatiquement et il n'y a pas de repli sur l'ancien emplacement.

Les images sont détectées à chaque récupération de la liste des systèmes : leur ajout, remplacement ou suppression dans le dossier configuré ne nécessite **ni réindexation ni redémarrage du serveur**. Le client actualise cette liste au démarrage et au retour depuis les jeux. L'API `/api/v1/systems` expose un champ `image` optionnel avec l'identifiant du fichier, sa taille et son SHA-256 ; le contenu est disponible via `/api/v1/files/{id}`. Le client récupère ces images via la même API et les conserve dans `cache_dir`. Elles apparaissent dans la grille ou le panneau latéral ; sans image ou en cas d'erreur, un emplacement de remplacement est affiché. Les fichiers absents, inaccessibles, sortant du dossier d'icônes après résolution des liens symboliques ou dépassant 8 Mio sont ignorés (avec essai du format suivant). Dimensions maximales de décodage côté client : 4096 × 4096.

Ces images servent à la navigation dans le store : elles ne sont pas ajoutées aux installations des jeux ni à leur `gamelist.xml`. Les jaquettes existantes ne changent pas.

## Installer le client sur Batocera

Une archive installable crée automatiquement la configuration, fournit la police et
ajoute **ROM Store** dans le menu **Ports**, sans Rust à installer sur Batocera.

Depuis un PC Linux avec Docker ou Podman :

```bash
bash scripts/build-batocera.sh --arch x86_64
```

Copier l'archive produite dans `dist/` sur Batocera, l'extraire, puis lancer :

```bash
python3 romstore-client-0.1.0-batocera-x86_64/install.py --server http://ADRESSE_DU_SERVEUR:8000
```

Utiliser `aarch64` pour un système ARM64. Les mises à jour conservent la configuration
et les ROMs existantes. **[Guide complet : copie SSH, compatibilité, mise à jour et diagnostic](docs/batocera.md).**

## Démarrage rapide sur Linux (développement)

Prérequis de compilation : Rust **1.90+**, compilateur C et `pkg-config`. Pour le client seulement : SDL2 **2.0.18+** et police TrueType ; `SDL2_ttf` et `SDL2_image` ne sont pas nécessaires. Le serveur cible Linux/Unix et embarque SQLite, sans Python, OpenSSL ni SDL à installer pour son exécution.

Sur Debian/Ubuntu, installer les dépendances natives :

```bash
sudo apt-get install build-essential pkg-config libsdl2-dev fonts-dejavu-core
```

Le `Makefile` à la racine permet de compiler et tester les deux applications :

```bash
make                 # Client et serveur en release
make PROFILE=dev     # Compilation de développement
make test-unit       # Tests unitaires du client et du serveur
make test            # Tests Rust unitaires et d'intégration (hors tests ignorés)
make help            # Toutes les cibles disponibles
```

`make client` et `make server` compilent séparément chaque binaire, dans
`client/target/release/` et `server/target/release/` par défaut. Les tests utilisent
le profil de test Cargo habituel. Les tests E2E ignorés restent à lancer explicitement
avec la commande décrite dans la section [Tests et qualité](#tests-et-qualité).

### 1. Lancer le serveur de démonstration

Depuis la racine du projet :

```bash
make run-server
```

- API : <http://127.0.0.1:8000/api/v1/health>
- Documentation interactive : <http://127.0.0.1:8000/docs>
- Catalogue SQLite de démonstration : `.romstore/test/catalog.sqlite3`, hors de la bibliothèque source.

Le dossier [`test/`](test/README.md) fournit cinq jeux fictifs sur trois plateformes,
dont un jeu multidisque, avec jaquettes PNG, icônes et `gamelist.xml`.
Les ROMs sont entièrement synthétiques : **elles ne sont pas jouables**.
Dans un autre terminal, `make run-client` ouvre le client avec la configuration
`test/client.yaml`, qui installe les téléchargements dans `.romstore/test/installed`.
Les données sont déjà fournies ; `make fixtures` permet de les régénérer avec Python 3.

Pour une bibliothèque réelle et un accès depuis le réseau local :

```bash
./server/target/release/romstore-server serve \
  --library /srv/roms \
  --database .romstore/catalog.sqlite3 \
  --system-icon .romstore/system-icons \
  --host 0.0.0.0 --port 8000
```

**Ne pas exposer ce serveur à Internet en l'état.** Il n'y a pas encore d'authentification. L'écoute est limitée à `127.0.0.1` par défaut. Ne distribuer que des fichiers pour lesquels vous disposez des droits nécessaires.

**Migration depuis Python :** arrêter l'ancien serveur, sauvegarder sa base SQLite,
puis lancer ce nouveau binaire avec les **mêmes** options `--library`, `--database`,
`--system-icon`, `--host` et `--port`. L'API v1, les identifiants, les révisions et le
cache d'empreintes restent compatibles ; aucune modification du client n'est requise.
L'ancien paquet Python et ses tests ont été remplacés par le crate et les tests Rust.
Le `.venv` n'est plus utilisé par le serveur. Voir **[déploiement, Docker et procédure de migration](docs/server.md)**.

#### Suivre le démarrage et l'indexation

Les journaux de progression sont activés **par défaut** au niveau `info`, au démarrage,
avec la commande `index` et lors d'un rechargement demandé depuis le client. Ils affichent :

- l'ouverture de SQLite et le chargement du cache d'empreintes ;
- le nombre de dossiers à examiner, la plateforme en cours et les entrées ROM trouvées dans son XML ;
- la progression toutes les **100 entrées ou environ 5 secondes**, les ROMs indexées et les empreintes réutilisées/calculées ;
- pour les fichiers d'au moins **256 Mio**, le début et la fin du calcul SHA-256 ; pour tout fichier lent, la progression en Mio, pourcentage et débit environ toutes les 5 secondes de lecture ;
- la publication SQLite, puis un bilan avec durée, ROMs installables/ignorées, fichiers, volume haché et avertissements.

Le nombre d'entrées annoncé vient des `gamelist.xml` : ce n'est pas un décompte de tous
les fichiers présents sur disque. Le total global est connu à la fin ; pendant l'analyse,
la progression est donnée par plateforme, sans relire tous les XML pour prédire un pourcentage global.

Le serveur ouvre l’accès HTTP sans attendre l’indexation, qui s’exécute **en arrière-plan**.
Le catalogue enregistré reste consultable jusqu’à la publication atomique du nouveau ;
au tout premier lancement, il reste vide pendant l’analyse. Le client surveille l’état
du serveur toutes les 3 secondes, affiche l’indexation en cours, désactive son raccourci
de rechargement et actualise le catalogue quand l’analyse se termine.

L’analyse peut être longue avec beaucoup de ROMs et de gros ISO : **les ROMs et
leurs médias doivent être lus pour calculer leur SHA-256**. Les démarrages suivants
réutilisent les empreintes des fichiers inchangés. Conserver la même base SQLite sur un
stockage persistant (volume dédié si le serveur tourne en conteneur) pour conserver ce
cache ; les nouvelles empreintes sont enregistrées à la fin de l'indexation.

Une petite **interface web de gestion** est disponible à la racine du serveur
(`http://adresse:8000/`) : état de l’indexation, bouton de relance et nombres de ROMs
totales/installables par système dans le dernier catalogue publié. Elle fonctionne
sans CDN ni connexion Internet. Les demandes du client et de la page web rejoignent
la même analyse lorsqu’elle est déjà en cours. Cette interface n’est pas authentifiée :
réserver l’accès au réseau de confiance.

La section **Anomalies du catalogue** permet de filtrer par système les entrées
ignorées (chemin XML, raison et numéro de ligne pour les nouveaux scans), les médias
manquants et les jeux non installables (nom, chemin et raison). Elle consulte le dernier
catalogue publié, sans relancer d’analyse. Les logs distinguent les compteurs de la
plateforme du cumul global et affichent au plus cinq exemples d’entrées ignorées et
cinq jeux non installables par plateforme ; le détail complet reste dans la WebUI.

Pour davantage de détails, ajouter **`--log-level debug` à la commande habituelle**, en
gardant les mêmes chemins `--library` et `--database`. Cela ajoute les fichiers en cours,
les réutilisations du cache et les raisons détaillées des entrées ignorées. Les niveaux
acceptés sont `debug`, `info` (défaut), `warning`, `error` et `critical` (alias d'`error`).
`RUST_LOG` permet aussi un filtrage avancé et prend priorité sur cette option s'il est défini.

Les logs horodatés sont écrits sur **stderr** : le JSON de la commande `index` reste seul
sur stdout. Ils sont donc également récupérables par les journaux habituels de systemd
ou du conteneur. L’indexation initiale et les rechargements s’exécutent en arrière-plan ;
le serveur accepte les requêtes HTTP et garde le catalogue précédent consultable
pendant l’analyse.

### 2. Configurer le client

Créer `client/config.yaml` (ignoré par Git ; l'extension `.yml` est également acceptée) :

```yaml
server_url: "http://127.0.0.1:8000"
roms_dir: ".romstore/installed"
cache_dir: ".romstore/cache"
font_path: "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
fullscreen: false
language: fr
games_view: grid
systems_view: list
frontend: none
auto_refresh: false
```

Langues disponibles : `fr` (français, par défaut), `en` (anglais), `es` (espagnol) et `de` (allemand). Modifier `language`, puis relancer le client. Les menus, raccourcis, états de téléchargement et messages courants sont traduits. Les noms et descriptions des jeux ne sont pas traduits automatiquement ; les diagnostics techniques bruts (serveur, système ou installation) restent dans leur langue d'origine. Une langue inconnue est refusée avec un message explicite.

**Migration depuis TOML :** renommer votre configuration en `.yaml` et remplacer les séparateurs `=` par `:` dans les champs ci-dessus. Conserver les valeurs et ajouter `language: fr` (ou une autre langue). Les fichiers TOML ne sont plus lus comme configuration client ; les `Cargo.toml` restent les manifestes de compilation du serveur et du client.

Les chemins relatifs sont résolus depuis le **répertoire de lancement**. `font_path` est facultatif : le client essaie cette police en premier, puis plusieurs emplacements DejaVu Sans, Noto Sans, Liberation Sans et FreeSans courants sur Fedora, Debian/Ubuntu et Batocera. Si nécessaire, il interroge aussi `fc-match` (Fontconfig), lorsqu'il est disponible. Un fichier absent, illisible ou invalide n'empêche pas d'essayer les suivants. La police retenue est indiquée dans le terminal ; si aucune ne convient, l'erreur liste les chemins essayés.

Le texte est rendu avec anticrénelage à une résolution adaptée à la sortie réelle
de la fenêtre (HiDPI et plein écran compris), avec suréchantillonnage des petites
tailles et filtrage linéaire. Les textures de texte sont régénérées si la densité
change au redimensionnement, sans changer les dimensions de l’interface. Aucun
réglage supplémentaire n’est nécessaire ; `font_path` reste utilisable pour choisir
une autre police TrueType/OpenType vectorielle.

Si les jaquettes signalent une erreur de droits, vérifier `cache_dir` : un chemin absolu vers un dossier accessible en écriture évite toute ambiguïté liée au lancement. Le cache est facultatif : si sa lecture ou son écriture échoue, le client télécharge et affiche l'image en mémoire. Les diagnostics indiquent le chemin absolu du cache ; une erreur de téléchargement est signalée séparément comme provenant du serveur. Aucun changement de permissions ni lancement en root n'est nécessaire pour contourner un cache inaccessible.

Sur Batocera, utiliser `/userdata/roms` pour `roms_dir` et un cache dans `/userdata/system/cache/romstore`. Un exemple est disponible dans [client/config.example.yaml](client/config.example.yaml).

#### Refresh du frontend à la fermeture

Pour actualiser automatiquement la liste des jeux de **Batocera** quand ROM Store
se ferme normalement, ajouter :

```yaml
frontend: batocera
auto_refresh: true
```

Le client attend la fin des écritures d’installation/désinstallation, libère sa
fenêtre et ses contrôleurs, puis appelle `http://127.0.0.1:1234/reloadgames`.
Il ne redémarre ni EmulationStation ni la machine. L’accès Web public de Batocera
n’a pas besoin d’être activé. `auto_refresh: false` désactive tout appel.
Les anciennes configurations gardent le comportement précédent (`frontend: none`,
`auto_refresh: false`) ; les nouvelles installations via le paquet Batocera activent
le refresh. Une mise à jour ne modifie pas une configuration existante.

Valeurs de `frontend` : `none`, `batocera`, `esde` (alias `es-de`), `recalbox`,
`emulationstation`, `custom`. Pour les frontends sans intégration native, il faut
fournir une commande de refresh **adaptée à l’installation locale** :

```yaml
frontend: recalbox
auto_refresh: true
refresh_command: ["/chemin/vers/mon-script-de-refresh", "argument"]
```

`refresh_command` remplace aussi l’appel natif Batocera si elle est renseignée.
C’est une liste exécutable/arguments, pas une chaîne shell : pas d’interpolation
ni de `sh -c` ajouté automatiquement. N’utiliser que des scripts de confiance.
Les appels HTTP et commandes ont un délai de 5 secondes ; un échec est journalisé,
sans annuler les installations ni empêcher la fermeture. Le refresh s’effectue
aussi après une installation CLI réussie, mais jamais avec `--help` ou `--smoke-test`.
Un arrêt brutal, crash ou SIGKILL ne permet pas de garantir son exécution.

**ES-DE :** ce frontend déconseille les modifications externes de ses gamelists
pendant son exécution ; le fermer avant de modifier sa bibliothèque, puis le relancer.
Un simple rescan ne recharge pas correctement ces modifications. Aucune commande de
redémarrage forcé n’est fournie par défaut.

#### Bibliothèque ES-DE et correspondances des systèmes

Le mode `frontend: esde` (alias `es-de`) installe les médias dans l’arborescence
ES-DE et génère un gamelist sans balises de médias. Les deux racines
`media_dir` et `gamelists_dir` doivent être renseignées explicitement pour correspondre
à votre installation ES-DE. Exemple de `config.yml` :

```yaml
server_url: "http://192.168.1.10:8000"
frontend: esde
auto_refresh: false
roms_dir: "/home/alice/ROMs"
media_dir: "/home/alice/ES-DE/downloaded_media"
gamelists_dir: "/home/alice/ES-DE/gamelists"
system_mapping:
  gamecube: gc
  sg1000: sg-1000
```

Les chemins sont des **racines**, sans le sous-dossier système. Ils peuvent être
sur des disques différents. Utiliser des chemins absolus, ou des chemins relatifs
au répertoire de lancement ; `~` et `$HOME` ne sont pas développés automatiquement.
Les chemins par défaut du cache, de la police et de l’interface restent inchangés.

Pour `gamecube` sur le serveur et `Batman Begins [GINX69].rvz`, cet exemple produit :

```text
/home/alice/ROMs/gc/Batman Begins [GINX69].rvz
/home/alice/ES-DE/gamelists/gc/gamelist.xml
/home/alice/ES-DE/downloaded_media/gc/marquees/Batman Begins [GINX69].png
/home/alice/ES-DE/downloaded_media/gc/covers/Batman Begins [GINX69].jpg
/home/alice/ES-DE/downloaded_media/gc/fanart/Batman Begins [GINX69].jpg
/home/alice/ES-DE/downloaded_media/gc/miximages/Batman Begins [GINX69].png
/home/alice/ES-DE/downloaded_media/gc/physicalmedia/Batman Begins [GINX69].png
```

Seuls les médias disponibles sur le serveur sont installés, sans conversion d’image.
Le nom correspond à celui de la ROM **sans sa dernière extension**, suivi de
l’extension du média en minuscules. Les sous-dossiers de la ROM sont reproduits
sous chaque catégorie. Pour les jeux multidisques, le nom du M3U sert de référence.

| Balise média serveur | Répertoire ES-DE |
|---|---|
| `marquee` (sinon `wheel`) | `marquees` |
| `boxart` (sinon `thumbnail`) | `covers` |
| `fanart` | `fanart` |
| `image` | `miximages` |
| `cartridge` | `physicalmedia` |
| `screenshot` | `screenshots` |
| `titleshot` | `titlescreens` |
| `boxback` | `backcovers` |
| `video` | `videos` |
| `manual` | `manuals` |

`map` et `bezel`, sans équivalent direct dans cette disposition, ne sont pas
téléchargés en mode ES-DE. Les images doivent être en PNG/JPG/WebP, les manuels en
PDF et les vidéos en MP4/MKV/AVI/WMV/MOV/WebM/M4V ; un format incompatible interrompt
l’installation sans écraser de fichiers locaux.

Le gamelist conserve `<path>./nom-de-la-rom.rvz</path>` relatif au **dossier des ROMs
du système**, même si le XML est ailleurs. Les métadonnées (`name`, `desc`, `rating`,
`releasedate`, `developer`, `publisher`, `genre`, `players`…) sont fusionnées sans
les chemins média Batocera. Les favoris, statistiques et autres entrées locales sont
préservés ; l’ancien XML est sauvegardé dans le dossier `.romstore` du système ROM.
La désinstallation retire la ROM et son entrée du gamelist externe, mais conserve
les médias et sauvegardes de jeu comme auparavant.

`system_mapping` fonctionne aussi avec Batocera et les autres frontends. Sans
correspondance explicite, le nom serveur est conservé. Ce mapping s’applique
uniquement aux chemins locaux (installation, détection, désinstallation et reprise),
pas aux identifiants des requêtes serveur. Il ne renomme ni ne déplace une bibliothèque
déjà installée : conserver la même configuration pour reprendre une opération.
`gamelists_dir` peut également déplacer les gamelists hors des ROMs sans activer ES-DE ;
`media_dir` n’est utilisé qu’en mode ES-DE. Sans ces options, le comportement Batocera
historique reste inchangé.

### 3. Compiler et ouvrir le client

```bash
cargo run --manifest-path client/Cargo.toml -- --config client/config.yaml
```

Avec `--config`, le client charge et sauvegarde exactement le fichier indiqué. Sans cette option, il utilise `$XDG_CONFIG_HOME/romstore/config.yaml` (ou `~/.config/romstore/config.yaml`) s'il existe ; sinon il démarre avec `http://127.0.0.1:8000`, `/userdata/roms` et le français, puis crée ce fichier lors de la première sauvegarde des réglages. Il ne découvre pas automatiquement `client/config.yaml` dans le dépôt.

| Action | Manette SDL GameController | Clavier |
|---|---|---|
| Naviguer | Croix ou stick gauche | Flèches |
| Choisir / ouvrir la fiche / valider son action | Bouton principal, SDL A | Entrée |
| Activer la multisélection / cocher un jeu | Maintenir SDL A (~650 ms) | Maintenir Entrée |
| Télécharger les jeux cochés (mode sélection uniquement) | SDL X | Espace |
| Rechercher des jeux | Clic stick gauche, SDL L3 | F |
| Aller directement à une lettre | R1 (bouton d'épaule droit SDL) | L |
| Retour (ne quitte jamais l'application) | Bouton secondaire, SDL B | Retour arrière |
| Ouvrir / fermer le menu | Start | Échap |
| Annuler la tâche sélectionnée dans les téléchargements | SDL A | Entrée |
| Effacer les tâches terminées de la liste des téléchargements | SDL X | Espace |

La lettre physique des boutons varie selon la manette. Les périphériques doivent être reconnus par SDL comme GameController. Pour une manette non reconnue, fournir un mapping via `SDL_GAMECONTROLLERCONFIG`.

L'interface adopte un thème bleu nuit avec accents menthe, cartes arrondies et sélection contrastée. Les plateformes et les jeux proposent chacun deux vues : **grille de visuels** ou **liste avec détails à droite**. Les champs YAML `games_view` et `systems_view` acceptent `grid` ou `list` (respectivement `grid` et `list` par défaut). Les deux réglages se trouvent dans **Start / Échap → Configuration**, et non plus directement dans le menu principal ; ils sont conservés au prochain lancement.

Le sous-menu **Configuration** expose tous les paramètres de `config.yaml` : URL du serveur, port, dossiers ROMs/médias/gamelists/cache, police, plein écran, langue, vues, frontend, refresh automatique, correspondances de plateformes et commande de refresh. **↑ ↓** sélectionne un champ, **← → / A** change une option ou ouvre son éditeur. Les collections `system_mapping` et `refresh_command` possèdent leur propre liste : ajout, modification et suppression des correspondances ou de chaque argument, sans saisie de JSON/YAML. Le port est un éditeur numérique de la partie port de `server_url` ; il n'ajoute pas de clé YAML supplémentaire.

**B / Retour arrière**, **Échap / Start** ou **Sauvegarder et revenir au menu** ferment le sous-menu et enregistrent **tous** les paramètres, y compris les valeurs par défaut. Dans un éditeur de texte, **B** annule uniquement cette saisie ; dans une liste de correspondances/arguments, il revient d'abord à Configuration. Une fermeture de fenêtre demande également la sauvegarde avant de quitter. Une configuration invalide (par exemple ES-DE sans dossiers obligatoires), un fichier en lecture seule ou une erreur d'écriture laisse le sous-menu ouvert avec un message, sans appliquer partiellement les changements. Si le fichier est modifié ailleurs pendant la saisie, **Recharger le fichier** permet d'abandonner explicitement le brouillon et de repartir de son contenu actuel.

La sauvegarde YAML utilise un fichier temporaire synchronisé puis un remplacement atomique ; les valeurs sont conservées, mais pas les commentaires ni la mise en forme manuelle. Les liens symboliques existants sont conservés et leur cible est mise à jour. Langue, police, plein écran et vues sont appliqués à la fermeture ; changer de serveur recharge la bibliothèque et ignore les réponses de l'ancien serveur. Les installations déjà en file gardent leur URL et leurs chemins d'origine ; les opérations suivantes utilisent les nouveaux réglages.

Le menu donne aussi un accès direct aux **téléchargements**, à la **recherche avancée**, au **rechargement du catalogue serveur** et à **Quitter**. Si `auto_refresh: true`, il propose également une **actualisation manuelle du frontend** avec la même commande locale que le refresh automatique. Cette action est refusée tant que des installations/désinstallations sont en attente ou en cours ; elle s'exécute en arrière-plan et ses erreurs sont affichées. **B / Retour arrière ne quitte jamais l'application**, même à l'accueil. Les anciens raccourcis V/R3, I/Y, F5/L1, Tab/X et Suppr sont retirés de la navigation principale.

Pendant un téléchargement, un encart **centré en haut** affiche le nom du jeu en cours, sa position **n / total** parmi les téléchargements encore listés, ainsi qu'une **barre de progression et son pourcentage**. La progression reste à 0 % tant que le volume à transférer n'est pas connu. L'encart ne montre ni les tâches en attente ni les désinstallations et disparaît quand aucun téléchargement n'est actif. Le détail de la file reste accessible par le menu ; son ouverture sélectionne automatiquement le téléchargement actif (à défaut, l'opération active ou la première en attente). **A / Entrée** y annule la tâche sélectionnée, **B / Retour arrière** revient à l'écran précédent, et **X / Espace** efface de la liste les tâches terminées, échouées ou annulées. Ce nettoyage ne supprime aucun fichier et conserve les opérations actives/en attente ; les compteurs reflètent ensuite la liste restante.

La navigation est **en boucle** dans les menus, les listes et les grilles : haut depuis le début rejoint la fin, et bas depuis la fin rejoint le début. Dans les grilles, le déplacement vertical garde la colonne lorsque possible (dernière case disponible si la dernière ligne est incomplète) ; gauche/droite bouclent aussi aux extrémités. Ce fonctionnement s'applique également à Configuration, aux éditeurs, à la recherche avancée, au sélecteur alphabétique et aux claviers à l'écran. Pour les ROMs, la boucle couvre tout le catalogue filtré, pas seulement les pages déjà chargées ; le saut vers une extrémité non chargée nécessite une requête réseau et conserve les coches de multisélection.

Les listes affichent les noms sans numérotation. Les plateformes sont triées sur leur nom mappé (par exemple **Nintendo Switch**, pas `switch`). **L / R1** ouvre un sélecteur alphabétique dans les deux vues : choisir une lettre ou un chiffre avec les directions puis **Entrée / A**, ou taper directement le caractère au clavier. **Retour arrière / B** annule. Le saut sélectionne le premier nom correspondant ; pour les ROMs, il couvre tout le catalogue, même les pages non chargées, et conserve la plateforme et les filtres en cours. Si aucun nom ne correspond, la sélection reste inchangée. Le défilement reste possible dans les deux sens après un saut.

Le client masque les plateformes dont le compteur `game_count` est à zéro
(compteur de jeux installables publié par le serveur), au démarrage comme après
chaque actualisation. Le catalogue et les compteurs de la WebUI serveur ne changent pas.

La grille comporte **5 colonnes** et deux lignes visibles, avec la jaquette et le nom dessous. Le jeu sélectionné est encadré ; haut/bas changent de ligne et gauche/droite de vignette. La liste se parcourt avec haut/bas et affiche à droite jaquette, capture, description, développeur et joueurs. Hors multisélection, un **appui court sur A / Entrée ouvre la fiche au relâchement**, sans lancer d'installation. Une nouvelle validation **depuis la fiche** lance l'installation si le jeu est absent, ou propose sa désinstallation s'il est déjà présent. Une opération en attente ou en cours ne peut pas être ajoutée en double ; l'installation des formats non pris en charge reste bloquée.

### Télécharger plusieurs ROMs

Dans la grille ou la liste des jeux, **maintenir A / Entrée environ 650 ms** active la multisélection et coche le jeu courant, sans ouvrir sa fiche. Ensuite, **A / Entrée** coche ou décoche chaque jeu. Un compteur et des cases indiquent la sélection ; **X / Espace** ajoute les jeux cochés à la file dans l'ordre de sélection. Les téléchargements sont exécutés un par un par le worker existant, pas simultanément. **B / Retour arrière** annule la sélection sans quitter le catalogue ni arrêter les téléchargements déjà lancés.

Les jeux non installables, déjà présents localement ou déjà en file ne sont pas ajoutés. Leur état est revérifié au lancement du lot pour éviter les doublons. Les coches reposent sur les identifiants des jeux : elles restent valables pendant le défilement, le changement de vue et le saut « **Allez à la lettre** ». Une nouvelle recherche, une réindexation ou un changement de serveur ou de dossiers ROMs annule la sélection devenue potentiellement obsolète. Une erreur de communication avec le worker conserve les jeux qui n'ont pas encore été ajoutés.

### Quitter pendant les téléchargements

L'entrée **Quitter** du menu et la fermeture de fenêtre demandent désormais confirmation s'il reste des opérations actives/en attente, ou un refresh en cours. **Continuer les téléchargements est sélectionné par défaut** : A/Entrée, B, Échap ou Start permettent de revenir sans annuler les tâches. Pour quitter réellement, choisir explicitement **Annuler et quitter** avec droite/bas puis valider. Les transferts sont alors annulés ; les écritures locales déjà engagées terminent leur phase de publication/restauration avant de libérer l'application. Répéter la fermeture de fenêtre ne vaut pas confirmation.

### Espace disque avant téléchargement

Avant les transferts de chaque jeu, le client vérifie l'espace libre via les API natives (`fs2`, Unix/macOS/Linux et Windows), sans lancer `df`, PowerShell ou une autre commande système. Il additionne les fichiers manquants **par volume**, même si ROMs, médias et gamelists utilisent différents dossiers d'un même disque. Les disques réellement séparés sont contrôlés séparément. Les fichiers déjà présents et vérifiés ne sont pas comptés une seconde fois.

Le calcul tient compte des unités d'allocation, du remplacement XML, de sa sauvegarde, du journal et d'une **marge de 16 Mio par volume**. L'espace est de nouveau contrôlé juste avant la requête HTTP de chaque fichier. Si l'espace manque ou ne peut pas être interrogé, aucun transfert de ce fichier n'est lancé ; la file affiche le volume, la quantité nécessaire et la quantité disponible lorsqu'elles sont connues. Le contrôle porte sur chaque installation lorsqu'elle démarre, pas sur une réservation préalable de tout le lot : un jeu ultérieur peut être refusé si les précédents ont rempli le disque. Un autre programme peut toujours consommer de l'espace pendant un transfert ; les erreurs d'écriture restent gérées sans écraser les fichiers existants.

Le catalogue utilise un **défilement infini**, sans pages à changer : à l'approche de la fin des jeux chargés, le client récupère automatiquement le lot suivant en arrière-plan. La sélection et les jeux déjà visibles restent accessibles pendant ce chargement. Le compteur indique la position dans le catalogue complet ; en cas d'erreur réseau, continuer à défiler relance le chargement. Les lots sont toujours limités à 50 jeux côté API, sans téléchargement intégral au démarrage. Seules les jaquettes proches de la sélection sont gardées en mémoire (20 maximum).

Les jeux dont la ROM est déjà présente dans **`roms_dir/<plateforme>/<chemin du jeu>`** portent une **coche ✓** sur la grille et dans la liste, avec un badge **INSTALLÉ** dans les détails. Pour un M3U, la playlist doit être valide et **tous ses disques doivent être présents**. Cela inclut les jeux installés avant le lancement du client. L'état est actualisé au chargement du catalogue, à l'ouverture de la fiche et à la fin d'une installation ou désinstallation, sans relancer le client. Il indique la présence des fichiers, pas une vérification d'intégrité ni la présence de tous les médias ; aucun gros fichier n'est haché pour afficher le badge. Les chemins non sûrs et les liens symboliques sous la racine ROMs ne sont pas considérés comme installés.

Les images se chargent en arrière-plan avec deux workers fixes pour les vignettes et un pour la sélection ou la fiche. Les changements rapides de sélection remplacent les requêtes en attente ; les réponses obsolètes sont ignorées. La grille de plateformes ne charge que ses dix visuels visibles, pas l'intégralité des systèmes. Les jaquettes portrait, carrées ou paysage conservent leurs proportions et leur contenu entier : leur cadre s'adapte au visuel, sans étirement ni recadrage. Une cartouche de remplacement apparaît si une jaquette manque. Les transferts restent présentés en cartes avec progression, débit et états distincts.

### Désinstaller un jeu

Ouvrir sa fiche, puis choisir **Désinstaller ce jeu** avec **A / Entrée**. Une fenêtre
indique le nom et le chemin de la ROM concernée. **Annuler est sélectionné par défaut** :
choisir explicitement **Désinstaller** avec droite/bas, puis valider. **B / Échap** ferme
la fenêtre sans suppression.

La désinstallation retire **uniquement la ROM principale et son entrée dans le
`gamelist.xml` local**. Tous les médias (même non partagés), sauvegardes, fichiers annexes
et caches sont conservés. Une sauvegarde du XML original est créée dans
`<plateforme>/.romstore/gamelist-backup-*.xml`. Aucune requête de suppression n'est
envoyée au serveur ; la désinstallation ne nécessite pas de connexion à son API.
Pour un jeu multidisque, seule la playlist M3U est retirée : **les disques sont
conservés**, car ils peuvent être partagés ou préexister à l'installation.
**Vérifier le chemin affiché : `roms_dir` est la racine réellement modifiée, y compris
si elle pointe sur un partage réseau. Ne pas la faire pointer sur une bibliothèque
source que vous souhaitez conserver intacte.**

L'opération est suivie dans la file et partage le même worker et verrou système que
les installations. Les doublons XML, chemins non sûrs, liens symboliques et ROMs
référencées par d'autres entrées sont refusés. Les badges se mettent à jour à la fin ;
la fiche propose alors à nouveau **Installer ce jeu**. Actualiser EmulationStation
pour refléter le retrait dans son menu.

Une ROM est temporairement déplacée dans `<plateforme>/.romstore/uninstall-*/rom`
avant publication du XML, puis supprimée après réussite. En cas d'échec, une
restauration sans écrasement est tentée. Si une interruption ou un conflit laisse ce
dossier, **le conserver** : `restore.json` indique le chemin d'origine de la ROM
récupérable ; les sauvegardes XML permettent aussi de retrouver ses métadonnées.
Ce fichier n'est pas un manifeste d'installation utilisable avec `--recover`.

### Recharger les ROMs disponibles

Après un ajout, retrait ou changement de ROM, média ou `gamelist.xml`, **Start / Échap → Recharger le catalogue serveur** demande une réindexation complète au serveur. Un message indique le travail en cours ; la navigation reste accessible. À la fin, les plateformes et le catalogue sont actualisés automatiquement, ainsi que les indicateurs de ROMs installées. La recherche, les critères avancés et leur portée sont conservés ; le catalogue repart de son début pour éviter de mélanger les anciens et nouveaux lots. Si la plateforme a disparu, le client revient aux plateformes. Les fichiers installés localement et les tâches de téléchargement ne sont pas supprimés.

La lecture du catalogue reste disponible pendant l'analyse et sa publication dans SQLite est atomique. Les demandes simultanées rejoignent le rechargement déjà en cours dans le processus serveur, sans lancer plusieurs analyses. Les empreintes des fichiers inchangés sont réutilisées. Les fichiers sources ne sont jamais modifiés. Les ROMs doivent toujours être référencées dans les `gamelist.xml` : ce rechargement ne crée pas d'entrées pour les fichiers non référencés.

**Depuis le serveur ou un script**, déclencher le même rechargement à chaud par HTTP, puis consulter son état :

```bash
curl -X POST http://127.0.0.1:8000/api/v1/catalog/reload
curl http://127.0.0.1:8000/api/v1/catalog/reload
```

Le POST répond immédiatement avec HTTP **202**. Le GET retourne `idle`, `running`, `completed` ou `failed`, avec le nombre de plateformes, jeux et diagnostics à la fin. Les détails d'exception restent dans les journaux du serveur. Le client SDL interroge cet état toutes les 3 secondes ; arrêter le client n'annule pas l'analyse côté serveur. Un échec est affiché et la même action du menu permet de réessayer.

Pour une réindexation locale en ligne de commande, l'outil existant reste disponible (utiliser les mêmes chemins que le serveur) :

```bash
./server/target/release/romstore-server index --library /srv/roms --database .romstore/catalog.sqlite3
```

Ces commandes ne nécessitent pas de redémarrage du serveur. Comme les autres routes, le rechargement HTTP n'est pas authentifié : réserver l'accès à un réseau de confiance. Redémarrer une fois le serveur et relancer le client après installation de cette mise à jour pour disposer de la nouvelle commande.

### Recherche globale ou par plateforme

**F / L3** ouvre la recherche : depuis l'accueil, elle porte sur toutes les plateformes ; depuis un catalogue, elle est limitée par défaut à cette plateforme. La touche **Plateforme** du clavier à l'écran bascule entre cette plateforme et toutes les plateformes. Le panneau de détail des résultats globaux indique le système de chaque jeu.

Au clavier, saisir le nom puis valider avec **Entrée**. À la manette, déplacer la sélection sur le clavier à l'écran avec le stick ou la croix ; **A** active la touche choisie (caractère, effacement ou **OK**). **B** ferme la saisie sans changer les résultats. **Échap / Start** ouvre le menu sans soumettre la saisie. Dans un champ de texte, Retour arrière efface le dernier caractère et Suppr vide la saisie. Valider un texte vide retire le filtre de nom tout en gardant la portée et les autres critères.

La recherche porte sur les noms des jeux, sans distinction de casse, et accepte les accents et caractères spéciaux saisis au clavier (256 caractères maximum). Elle n'est envoyée au serveur qu'à la validation. Le filtre et sa portée sont conservés pendant le défilement infini, les changements de vue et l'aller-retour sur une fiche ; les résultats suivants se chargent progressivement comme dans le catalogue.

### Recherche avancée

**Start / Échap → Recherche avancée** ouvre un formulaire pilotable à la manette : plateforme avec **← →**, valeur d'un champ avec **A** puis le clavier virtuel ou physique. Le formulaire propose nom, année, joueurs, genre, développeur, éditeur et note, puis découvre les autres métadonnées indexées : région, langue, description, champs personnalisés, références de médias, etc. Les champs locaux volontairement exclus par l'indexeur (`favorite`, `hidden`, compteurs de jeu…) ne sont pas importés et ne figurent pas dans ces filtres.

Les critères non vides se combinent avec **ET**. Par défaut, le texte est recherché littéralement sans distinction de casse ; `=valeur` exige une égalité et `>=nombre`, `<=nombre`, `>nombre`, `<nombre` comparent les nombres. **Année** utilise les quatre premiers caractères de `releasedate`. **Joueurs : 2** accepte aussi `1-4` ou `2+`. La note conserve l'échelle du gamelist, généralement 0–1. Choisir **Appliquer les filtres** en bas du formulaire ; **Effacer tous les filtres** prépare une recherche sans critères. **B** annule sans modifier les résultats.

Les champs année et joueurs utilisent un **pavé numérique sans lettres**, avec les opérateurs utiles aux filtres (`=`, `<`, `>`, `-`, `+`). La note utilise un pavé décimal avec le point. Le port du serveur n'accepte que les chiffres (1–65535, ou vide pour le port par défaut du protocole). Les caractères alphabétiques sont aussi refusés au clavier physique et au collage dans ces champs. Les saisies textuelles gardent un clavier complet avec une touche `abc / ABC` pour les majuscules.

Le filtrage est effectué sur l'ensemble du catalogue serveur avant comptage et pagination. Les critères restent actifs lors du saut alphabétique, du défilement et du rechargement. **Cette recherche nécessite de mettre à jour le serveur et le client** : `GET /api/v1/games/fields` expose les champs, et `GET /api/v1/games?filters={...}` accepte un objet JSON champ/critère (32 critères maximum, 256 caractères par valeur). L'API existante reste compatible lorsque `filters` est absent.

### Jaquettes et mix-images

Les médias sont lus depuis les références du `gamelist.xml`. Le format Batocera suivant est directement pris en charge, sans ajouter de balise `screenshot` :

```xml
<game>
  <path>./jeu.nes</path>
  <name>Mon jeu</name>
  <image>./images/jeu-image.png</image>
  <marquee>./images/jeu-marquee.png</marquee>
  <thumbnail>./images/jeu-thumb.png</thumbnail>
  <fanart>./images/jeu-fanart.jpg</fanart>
</game>
```

La **fiche d’un jeu affiche uniquement le média `image`**, adapté aux mix-images qui
combinent déjà capture, jaquette et logo. Un grand panneau à gauche conserve l’image
entière et ses proportions, sans découpe ni étirement ; description, métadonnées et
action d’installation/désinstallation sont regroupées à droite. Il n’y a plus de
jaquette, logo, fond fanart ou capture séparés, ni de libellé « Capture d’écran » dans
la fiche. Si `image` manque, un emplacement neutre est affiché.

Dans le catalogue, `thumbnail` reste la jaquette, `image` l’aperçu et `marquee` le
logo du panneau latéral en vue liste. Le média `fanart` reste transféré s’il est
référencé, mais n’est plus chargé pour l’affichage. Les noms longs, espaces,
apostrophes et esperluettes sont conservés : le XML contient `&amp;`, tandis que le
fichier sur disque contient bien `&`.

Pour les autres bibliothèques, `boxart` reste un repli de jaquette si `thumbnail` manque ; dans le catalogue en vue liste, `screenshot` et `titleshot` sont des replis d’aperçu si `image` manque. La fiche utilise seulement `image`. Une capture ou un logo ne remplace jamais une jaquette absente. Tous ces médias sont facultatifs : leur absence ne bloque ni la navigation ni l'installation. Ils sont téléchargés et ajoutés au XML local avec contrôle d'intégrité et de chemins.

Après modification des références XML des **jeux**, réindexer le catalogue comme auparavant. Les images des **plateformes** restent détectées à la demande, sans réindexation.

Les tâches d'installation sont exécutées une par une ; la navigation reste disponible. L'annulation est observée entre les lectures réseau ; une lecture bloquée expire après 15 secondes. La publication des fichiers et la mise à jour XML, une fois engagées, se terminent sans annulation intermédiaire.

Une fermeture normale du client demande l'annulation des tâches et attend la fin des écritures avant de quitter.

## Installation sans interface et récupération

L'API `/api/v1/games` fournit les identifiants des jeux. Installer un jeu sans ouvrir de fenêtre :

```bash
cargo run --manifest-path client/Cargo.toml -- \
  --config client/config.yaml --install IDENTIFIANT_DU_JEU
```

Après une interruption, relancer la même installation réutilise les fichiers déjà présents **uniquement si taille et SHA-256 correspondent**. Les données partielles ne sont pas reprises octet par octet.

Un journal est conservé dans `<roms_dir>/<système>/.romstore/<identifiant>.json` tant que l'installation n'est pas terminée. Pour reprendre exactement ce manifeste :

```bash
cargo run --manifest-path client/Cargo.toml -- \
  --config client/config.yaml \
  --recover /chemin/roms/nes/.romstore/IDENTIFIANT.json
```

Les fichiers de cette révision doivent encore être disponibles sur le serveur. Sinon, corriger la bibliothèque, réindexer et installer la version courante. Aucun fichier local différent n'est écrasé automatiquement.

## Réindexation et diagnostics

Le serveur indexe la bibliothèque au démarrage. Après modification de la source :

```bash
./server/target/release/romstore-server index --library /srv/roms --database .romstore/catalog.sqlite3
```

La commande publie le nouvel index en une transaction SQLite et affiche les anomalies : XML invalide, média absent, doublon ou chemin interdit. Un serveur actif utilisant la même base voit le nouvel index sans redémarrage. Les empreintes des fichiers inchangés sont réutilisées.

## Attention au `gamelist.xml` local

- Les entrées des autres jeux, attributs et champs locaux inconnus sont conservés.
- Favoris, statistiques, masquage et dates de parties locales ne sont pas remplacés par ceux du serveur.
- Les mises à jour sont idempotentes par chemin de ROM normalisé.
- Une sauvegarde de chaque XML remplacé est conservée dans `<système>/.romstore/gamelist-backup-*.xml`.
- Un XML mal formé ou non UTF-8 est refusé, pas réinitialisé.

**Pour les premiers essais sur Batocera, arrêter EmulationStation avant les installations.** Son processus peut lui-même réécrire le XML, et les verrous du client ne sont pas partagés avec lui. La détection des modifications concurrentes réduit les risques mais ne garantit pas une transaction avec EmulationStation. Rafraîchir ensuite la liste des jeux ou redémarrer son interface. Aucun redémarrage automatique n'est exécuté.

## Tests et qualité

Le pipeline **GitLab CI/CD** teste et compile les deux applications
sur Linux x86_64. Les tags `client-vX.Y.Z` et `server-vX.Y.Z` compilent, testent et
publient indépendamment le composant concerné avec son archive et sa somme SHA-256.
Les versions des deux applications peuvent différer. Sur `main`, une pipeline enfant
propose les jobs manuels `fix`, `minor` et `major` pour chaque composant modifié
depuis son dernier tag (trois à six jobs), afin de préparer et pousser la release
directement depuis GitLab. Le client affiche sa version
dans un petit encadré de l'en-tête, automatiquement issu de son `Cargo.toml`.
Voir [la configuration du runner et la procédure de release](docs/ci.md).

```bash
cargo test --locked --manifest-path server/Cargo.toml
cargo clippy --locked --manifest-path server/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path server/Cargo.toml -- --check
cargo test --locked --manifest-path client/Cargo.toml
cargo clippy --locked --manifest-path client/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path client/Cargo.toml -- --check
```

Test de bout en bout avec le véritable serveur HTTP et le binaire Rust :

```bash
cargo build --locked --manifest-path client/Cargo.toml
ROMSTORE_CLIENT="$PWD/client/target/debug/romstore-client" \
  cargo test --locked --manifest-path server/Cargo.toml --test e2e -- --ignored
```

Ces tests créent leurs propres bibliothèques synthétiques, sans dépendre de
`fixtures/library` ni toucher aux ROMs utilisateur. Une référence JSON produite par
l'ancien serveur vérifie la compatibilité des identifiants et révisions ; un cas à
10 050 entrées vérifie pagination et cache à chaud.

La CI et les releases n'utilisent pas Python : `cargo-release` prépare les versions,
`git-cliff` génère les notes et GitLab publie les archives. `make release-tools`
installe les outils épinglés ; `make release-client TYPE=fix|minor|major` et
`make release-server TYPE=fix|minor|major` préparent le commit et le tag sans push.
`make test-ci` teste les scripts Bash associés. Python reste nécessaire uniquement
pour régénérer les données synthétiques avec `make fixtures`.

Test de démarrage et de rendu SDL sans écran :

```bash
SDL_VIDEODRIVER=dummy client/target/debug/romstore-client --smoke-test
```

Pour examiner les écrans de démonstration sans serveur ni ROM commerciale :

```bash
SDL_VIDEODRIVER=dummy client/target/debug/romstore-client \
  --smoke-test --screenshots .romstore/screenshots
```

Le test SDL couvre 50 scènes : vues grille/liste, défilement, fiches, médias manquants, file et notification de téléchargement (progression partielle, inconnue et complète, noms longs), erreurs et chargements, recherches, claviers textuels/numériques, désinstallation, sélecteur alphabétique, menu, sous-menu Configuration, multisélection, confirmation de fermeture, manque d'espace disque, sélection du téléchargement actif et nettoyage de la liste. Les captures PNG sont facultatives et écrites uniquement avec `--screenshots`. Ce test couvre l'initialisation et le rendu, pas l'ergonomie ou une manette physique. Les tests Rust couvrent aussi la persistance des réglages, les appuis courts/longs, les lots, la confirmation sans annulation prématurée, les contrôles d'espace, la navigation en boucle et le suivi des tâches après nettoyage.

## Limites de cette version

- Pas encore de résolution des formats `.cue`, `.gdi`, `.ccd`, `.mds`, ni de playlists imbriquées : affichés mais non installables, y compris lorsqu'ils sont référencés dans un M3U.
- Pas encore de filtres avancés (genre, année…), de configuration à l'écran ou de cache de catalogue hors ligne.
- Pas de reprise HTTP Range ni de persistance de la file d'attente. Les journaux d'installation restent récupérables manuellement.
- La fiche affiche les images PNG/JPEG/WebP/PNM, pas les vidéos ou PDF ; les médias référencés pris en charge sont tout de même téléchargés.
- La police n'est pas encore embarquée : une police utilisable doit être installée (détection automatique ou `font_path`).
- Pas encore de paquet ou de lanceur Batocera validé : la version, l'architecture cible et les bibliothèques disponibles restent à confirmer sur le matériel réel.
