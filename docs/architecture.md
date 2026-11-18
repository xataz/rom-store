# Architecture et garanties du MVP

## Serveur

Le crate autonome `server/` est écrit en Rust (MSRV 1.90), sans dépendance au client
SDL ni à un interpréteur Python. `server/src/` contient :

- `config.rs` : racines validées, base extérieure à la bibliothèque et icônes séparées.
- `catalog.rs` : catalogue Rusqlite, cache d'empreintes et publication transactionnelle.
- `models.rs` : contrats Serde et schémas Utoipa/OpenAPI.
- `api.rs` : routeur Axum, erreurs JSON, requêtes asynchrones et flux de fichiers.
- `reload.rs` : tâche Tokio et état du rechargement, regroupement des demandes.
- `paths.rs` : chemins, ouverture de fichiers réguliers et signatures Unix.
- `playlist.rs` : lecture bornée des M3U et résolution des chemins de disques.
- `identity.rs` : identifiants, casefold Unicode ICU et format historique des révisions.
- `xml.rs` : décodage XML (UTF-8/UTF-16, encodages déclarés dont ISO-8859-1).
- `progress.rs` : hachage par blocs et logs de progression.
- `main.rs` : CLI Clap `serve` / `index`, logs Tracing et arrêt sur SIGINT/SIGTERM.

Les opérations SQLite et les scans synchrones passent par `spawn_blocking` ; les
handlers ne bloquent pas les workers asynchrones. Le streaming utilise
`tokio::fs::File` et `ReaderStream`, borné à la taille vérifiée du fichier. SQLite est
embarqué au build, sans dépendance à une bibliothèque SQLite système à l'exécution.

Le serveur ne fournit aucune route d'écriture sur la bibliothèque ni de lancement de commande arbitraire. La commande locale `index` et la réindexation distante calculent le nouveau catalogue, puis remplacent ses tables dans une transaction. Les lecteurs voient l'ancien ou le nouvel index, jamais un état partiellement publié. En mode `serve`, le port HTTP est ouvert avant le lancement de l’indexation en arrière-plan ; celle-ci partage son gestionnaire `Reload` avec l’API. Le catalogue enregistré est immédiatement consultable (vide au premier lancement). Un échec du scan ne termine pas le serveur. La commande `index` reste synchrone.

`POST /api/v1/catalog/reload` répond 202 et programme une tâche de fond Tokio ; `GET` sur la même route renvoie l'état (`idle`, `running`, `completed`, `failed`). Un mutex protège l'état : les demandes reçues pendant l'exécution rejoignent l'opération existante. Un verrou fichier à côté de SQLite empêche aussi deux processus Rust de réindexer cette même base simultanément. Les erreurs et paniques de la tâche produisent l'état failed et autorisent une nouvelle tentative. Les comptes rendus exposent les nombres de systèmes, jeux et diagnostics ; les erreurs détaillées sont journalisées uniquement côté serveur. L'état de la tâche est en mémoire, non persistant après redémarrage. Cette route d'administration reste réservée au réseau de confiance, comme toute l'API actuellement sans authentification.

Les identifiants de jeux dépendent du système et du chemin relatif ; ceux des fichiers dépendent aussi de leur SHA-256. Une révision de jeu couvre les métadonnées et les fichiers du manifeste. Les empreintes sont calculées pendant l'indexation et mises en cache par signature de fichier : périphérique, inode, taille, mtime et ctime.

La migration depuis Python conserve les tables/payloads, les signatures et le calcul
des révisions (`json.dumps(sort_keys=True, ensure_ascii=True)` historique, distinct du
JSON compact HTTP). Un jeu de référence généré par l'ancien serveur vérifie cette
compatibilité. `user_version=1` marque la reconstruction initiale de l'index de recherche
avec ICU ; cela ne supprime ni les payloads ni le cache. Les tests de migration et de
charge utilisent des fichiers temporaires, pas la bibliothèque réelle. Les anciennes
sources Python/FastAPI et leurs tests ont été remplacés, sans toucher aux bases utilisateur.

L'indexation utilise la cible Tracing `romstore_server::catalog`. La CLI configure des logs
horodatés sur stderr (`--log-level info` par défaut), sans polluer le JSON de `index`
sur stdout. Ouverture de la base, chargement du cache, lecture de chaque XML et
publication SQLite sont distingués. Les compteurs de progression sont propres à chaque
indexation et publiés toutes les 100 entrées ou environ 5 secondes ; les totaux XML,
ROMs effectivement indexées, installables, entrées ignorées, fichiers et avertissements
restent distincts. Les erreurs individuelles sont détaillées en debug, avec un bilan
warning par plateforme concernée, plutôt qu'une ligne warning par média manquant.

Le SHA-256 est calculé par blocs de 1 Mio : cela conserve une mémoire bornée et permet
d'annoncer l'avancement des lectures longues, sans changer l'empreinte ni la vérification
de signature avant/après lecture. Les fichiers d'au moins 256 Mio sont annoncés dès le
début ; les lectures lentes fournissent volume, pourcentage et débit environ toutes
les 5 secondes, lorsque la lecture rend la main. Aucun thread de progression ni
nouveau parcours intégral des fichiers n'est ajouté. Le bilan distingue les empreintes
réutilisées et nouvellement calculées ; les références multiples à un même média ne
gonflent pas le nombre de fichiers réutilisés. Ces logs sont communs au démarrage, à
la commande locale `index` et au rechargement distant.

Les ROMs et médias de jeux doivent rester à l'intérieur de leur système après résolution des liens symboliques. Les icônes de plateformes doivent rester dans leur propre racine `--system-icon`. Les systèmes et XML symlinks sont refusés ; un lien de fichier interne restant dans sa racine peut être lu. Les DTD et entités déclarées sont interdites par RoXMLTree ; la lecture XML brute est bornée à 16 Mio. Les fichiers résolus sont ouverts avec O_NOFOLLOW/O_NONBLOCK puis vérifiés réguliers. Le streaming vérifie la signature de l'inode ouvert ; le client contrôle ensuite l'empreinte complète. Une modification ultérieure d'un fichier en cours de lecture est donc détectée par le client, sans publication d'un fichier corrompu.

La pagination est exécutée dans SQLite : seuls les jeux de la page demandée sont désérialisés. Des index système/nom et nom global assurent un tri Unicode stable sans trier tout le catalogue pour chaque lot ; le total et la page partagent le même snapshot de lecture. Les filtres absents ne sont pas ajoutés aux prédicats SQL, afin de conserver l'usage des index et d'éviter un casefold intégral pour un simple comptage non filtré.

## API v1

| Route GET | Rôle |
|---|---|
| `/api/v1/health` | Version et disponibilité |
| `/api/v1/systems` | Systèmes, nombre de jeux installables et image optionnelle (contrat File) |
| `/api/v1/games` | `system`, `search`, `offset`, `limit` (1–200) |
| `/api/v1/games/{id}` | Jeu, métadonnées et fichiers |
| `/api/v1/games/{id}/manifest` | Version 1, destinations, révision, tailles et empreintes |
| `/api/v1/files/{id}` | Octets du fichier ; ETag et précondition If-Match |
| `/api/v1/catalog/reload` | État du dernier rechargement (POST sur cette route pour le déclencher, réponse 202) |
| `/api/v1/catalog/summary` | Compteurs totaux et installables par système dans le catalogue publié, sans lecture des ROMs/icônes |
| `/api/v1/catalog/issues` | Avertissements et jeux non installables, paginés (limite 200), filtre `system` facultatif |

La racine `/` sert une petite page de gestion embarquée (`admin.html`), sans CDN : suivi du scan, bouton de relance désactivé pendant l’analyse et compteurs par système, rafraîchis toutes les 3 secondes. Les noms sont insérés comme texte, pas comme HTML. L’interface et ses routes restent sans authentification et réservées au réseau de confiance.
La section Anomalies combine les diagnostics persistés et les jeux non installables,
lus dans une même transaction SQLite ; le chemin, le nom disponible et la raison
sont affichés avec filtre par système et pagination. Un jeu peut avoir plusieurs
anomalies. Les nouvelles entrées XML ignorées distinguent doublons, fichiers absents,
permissions et autres erreurs de chemin/lecture, avec numéro de ligne. Les logs
mentionnent le fichier XML réellement lu, séparent progression locale et cumul global
et limitent les exemples d’entrées ignorées/non installables à cinq de chaque par plateforme.

Les erreurs métier utilisent `{"error":{"message":"…"}}`. Un fichier absent de l'index retourne 404 ; une source modifiée nécessite une réindexation (409) ; une précondition If-Match incorrecte retourne 412. Les chemins absolus du serveur ne figurent pas dans ces erreurs.

Les M3U/M3U8 UTF-8 sont résolus relativement à leur dossier, sans réécriture de leur
contenu. Le manifeste v1 contient exactement une entrée `rom` (la playlist), les
disques uniques dans l'ordre de première référence avec le rôle `disk`, puis les
médias. Les destinations restent relatives à la plateforme. Un `../` interne est
normalisé ; sorties de plateforme, URLs, chemins réservés, playlists imbriquées et
disques à dépendances sont refusés. La lecture est limitée à 1 Mio et 240 références.
La liste lue doit correspondre au SHA-256 de la playlist indexée. La révision couvre
aussi tous les disques, donc leur modification est détectée même si le M3U ne change
pas. Les M3U invalides ou incomplets et les autres formats à dépendances connues
restent visibles avec `installable=false` et une raison explicite. Un ancien client
qui ne connaît pas le rôle `disk` refuse ces manifestes sans publication de fichiers.

## Client

`config.rs` charge une configuration YAML (`.yaml` ou `.yml`) avec valeurs par défaut et refus des champs inconnus. Le champ `language` accepte `fr`, `en`, `es` et `de` ; `fr` est la valeur par défaut. `i18n.rs` charge paresseusement les dictionnaires YAML de `client/locales/`, embarqués dans le binaire. Les traductions sont partagées en lecture seule et n'exigent aucun fichier externe à l'exécution. Les tests contrôlent la parité des clés, les paramètres de formatage et la présence des clés utilisées dans l'interface. Les métadonnées des jeux et les diagnostics bruts restent inchangés.

Le thread principal possède SDL, la fenêtre, les textures et les contrôleurs. Les lectures du catalogue et les images sont effectuées par des tâches de fond ; un numéro de génération évite qu'une ancienne réponse remplace un écran plus récent. La file d'installations a un seul consommateur.

La façade `Api::systems()` élimine les systèmes dont `game_count == 0`, à chaque
lecture initiale ou actualisation. Le serveur et sa WebUI conservent leurs données.
`frontend.rs` gère le refresh local facultatif à la fermeture normale : après attente
des opérations locales et libération de SDL, Batocera reçoit un GET loopback sur
`/reloadgames` (sans proxy ni redirection). Les autres frontends (`esde`, `recalbox`,
`emulationstation`, `custom`) nécessitent une commande locale explicite si le refresh
est activé. `refresh_command` est une liste exécutable/arguments, jamais une commande
shell implicite ni une donnée du serveur. Le délai HTTP/processus est de 5 secondes ;
les erreurs sont journalisées sans faire échouer une installation terminée. Les
installations CLI réussies utilisent le même mécanisme ; aide et smoke test ne le
déclenchent jamais. Par défaut : `frontend: none`, `auto_refresh: false` ; seuls les
nouveaux paquets installés pour Batocera activent les options dans leur configuration.

`layout.rs` résout les chemins locaux à partir de `system_mapping` sans modifier les
manifestes serveur ni leurs journaux de reprise. `frontend: esde` exige `media_dir`
et `gamelists_dir` : les médias sont associés au chemin de la ROM sans extension,
répartis par catégorie et les balises média sont retirées de l’entrée XML fusionnée.
La détection de présence et la désinstallation utilisent le même mapping ; le chemin
XML reste relatif au système ROM. Le verrou et les journaux restent dans le dossier
ROM `.romstore`, les gamelists peuvent être publiés ailleurs. Les téléchargements
sont préparés dans des fichiers cachés sur leur système de fichiers de destination
pour permettre une publication atomique même lorsque les médias sont sur un autre
disque. Les sommes de contrôle, le refus d’écrasement et les sauvegardes XML restent
actifs. Les médias ne sont toujours pas supprimés lors d’une désinstallation.

`api.rs` fournit une façade synchrone pour ces workers, adossée à un runtime Tokio et au client HTTP asynchrone Reqwest. Les appels JSON expirent après 20 secondes ; les connexions après 8 secondes ; chaque lecture d'un fichier après 15 secondes d'inactivité. Un transfert complet est limité à 24 heures. Les redirections sont refusées. Le thread SDL ne télécharge ni ne calcule les empreintes des ROMs.

L'action « Recharger le catalogue serveur » du menu lance un POST dans un worker. Indépendamment de cette action, le client surveille l’état par GET toutes les 3 secondes, sans limite globale de durée : il détecte donc aussi les indexations du démarrage et de la page web. Les requêtes HTTP restent courtes et ne bloquent jamais le rendu SDL. Une seule requête de suivi est en vol à la fois ; le menu indique l'analyse en cours et une nouvelle demande est ignorée pendant celle-ci. Une erreur réseau transitoire conserve le dernier état en cours et le suivi reprend automatiquement. À la fin, les systèmes sont relus, les générations précédentes invalidées et le catalogue rechargé depuis le début avec les filtres actifs. Les téléchargements locaux continuent. Une fiche ouverte pendant l’analyse revient au catalogue, ou aux plateformes si le système a disparu ; la file reste ouverte si elle était affichée.

`ui.rs` rend les textes avec Fontdue et les textures SDL, sans dépendance SDL2_ttf. Une police locale est nécessaire pour cette version. Les jaquettes sont vérifiées puis décodées hors du thread SDL avec limites de taille et de dimensions.

`views.rs` regroupe la présentation des quatre écrans, séparément de la navigation et des opérations réseau. Le bandeau contient l'accès au menu et, pendant une installation, un encart en haut à gauche avec uniquement le téléchargement actif et sa position parmi les installations de la session (hors désinstallations). Le fond dégradé est calculé une seule fois dans une texture ; les panneaux arrondis et les barres de progression utilisent directement SDL, sans SDL_gfx. Les textures de texte sont réutilisées. Le rendu conserve un espace logique de 1280 × 720. Les cadres de jaquettes épousent leurs proportions, sans découpe ni étirement. La fiche réserve un panneau de 660 × 436 à une seule mix-image, et une colonne à droite pour les informations et l’action. `smoke.rs` fournit 40 scènes synthétiques sans accès réseau, incluant les sous-menus de configuration et les claviers numériques.

Les images de plateformes (`<system-icon>/<identifiant>.png`, avec variantes JPEG/WebP/PNM) sont découvertes à chaque appel à `/api/v1/systems`, sans réindexation des jeux. `--system-icon` sélectionne le dossier ; par défaut il se trouve à côté de la base SQLite sous le nom `system-icons`. L'ancien `<système>/system.png` n'est plus consulté. Leurs descripteurs sont conservés dans une table SQLite distincte, indépendante de la publication du catalogue ; les empreintes des images inchangées sont réutilisées par signature. Les identifiants utilisent un espace de noms distinct (`system-icon`) pour éviter les collisions avec les médias des jeux. Les fichiers absents, inaccessibles, sortant du dossier d'icônes ou dépassant 8 Mio sont ignorés. Les images sont servies par la même route de fichiers sécurisée, en choisissant la racine d'icônes uniquement pour les descripteurs qui en proviennent, sans être incluses dans les manifestes des jeux. Les anciens descripteurs sans cette provenance ne sont pas utilisés pour ouvrir des icônes. Le client actualise les systèmes au démarrage et au retour depuis les jeux. En liste, une seule texture d'aperçu est conservée ; en grille, les dix plateformes visibles sont chargées. Le cache vérifie le SHA-256 et une génération distincte écarte les aperçus obsolètes.

Jeux et plateformes proposent une grille de cinq colonnes et deux lignes visibles, ou une liste avec panneau latéral. `games_view` et `systems_view` sont modifiés indépendamment dans Configuration et persistés à sa fermeture. Les seules actions directes hors directions sont A/Entrée, B/Retour arrière, F/L3 (recherche), L/R1 (lettre) et Start/Échap (menu). B ne quitte jamais l'application ; la sortie passe par l'entrée Quitter du menu ou la fermeture de la fenêtre. Dans le catalogue, A/Entrée ouvre uniquement la fiche. La validation depuis la fiche installe un jeu absent ou propose de désinstaller un jeu présent. La confirmation destructive capture le jeu cible, affiche son chemin et sélectionne Annuler par défaut. Un rechargement du catalogue ou l'ouverture du menu la ferme sans confirmer. Dans la file, A/Entrée annule la tâche sélectionnée.

`menu.rs` présente Configuration, les téléchargements, la recherche avancée, la réindexation serveur, le refresh manuel conditionné par `auto_refresh` et Quitter. `settings.rs` contient un brouillon de tous les champs de `Config`, avec éditeurs structurés pour les correspondances et les arguments. Les vues se règlent uniquement dans ce sous-menu. Retour/Start/Échap ou la fermeture de fenêtre valident et sauvegardent avant de fermer Configuration. Un échec conserve le brouillon ouvert et la configuration active inchangée. Le refresh local utilise un worker et la même commande de confiance que le refresh automatique ; il est refusé tant que des opérations sont en attente ou en cours et bloque l'ajout d'opérations locales jusqu'à sa réponse. La fermeture attend aussi ce worker.

`Config` est sérialisable ; la sauvegarde YAML valide l'ensemble des dépendances entre champs, détecte les changements externes pendant la saisie et utilise un fichier temporaire synchronisé suivi d'un renommage atomique. Les commentaires YAML ne sont pas préservés. Un fichier en lecture seule ou un lien sans cible est refusé ; un lien existant est suivi sans le remplacer. Le chemin de `--config` est conservé, ou `$XDG_CONFIG_HOME/romstore/config.yaml` / `~/.config/romstore/config.yaml` est chargé et utilisé pour les sauvegardes. Une génération de connexion distingue les suivis de réindexation des anciens serveurs. Chaque requête d'installation/désinstallation capture son propre `Api` et `Config` : les modifications de réglages n'altèrent pas les tâches déjà en file. Les workers d'images sont recréés si serveur/cache change ; langue, police et plein écran sont actualisés dans la boucle SDL après la sauvegarde.

Le clavier de `search.rs` possède des modes texte, entier, filtre entier et filtre décimal. Les modes numériques excluent les lettres de la grille, des événements SDL TextInput et du collage ; seuls les filtres autorisent les opérateurs, et seul le mode décimal propose un point. Le mode texte fournit une bascule de casse et conserve jusqu'à 4096 caractères pour les chemins/arguments de configuration ; les recherches restent limitées à 256 caractères.

`artwork.rs` utilise deux workers permanents pour les vignettes (256 × 256 maximum) et un worker pour les visuels de sélection et de fiche (640 × 640 maximum). Les files en attente sont remplacées au changement de sélection, sans multiplication des threads pendant la navigation. Les générations écartent les réponses obsolètes ; le catalogue limite les requêtes et textures à une fenêtre de 20 jaquettes autour de la sélection, indépendamment du nombre de jeux déjà chargés. Les textures encore utiles sont conservées pendant le défilement. Logo et aperçu du catalogue sont identifiés par fichier avant affichage. La fiche ne demande que le fichier de rôle `image`, sans téléchargement séparé des jaquettes, captures, logos ou fanart pour son rendu. Le retour depuis une fiche conserve galerie et sélection.

Le catalogue client cumule progressivement les lots de 50 jeux de l'API dans une seule liste, sans pagination visible. À moins de 20 jeux de la fin chargée, une seule requête supplémentaire est autorisée à la fois. Les lots sont ajoutés sans réinitialiser la sélection ni bloquer la navigation. Une génération de catalogue distincte des générations d'écran permet de recevoir un lot pendant une visite de fiche ou de la file, tout en ignorant les anciennes recherches. Une erreur conserve les jeux disponibles et autorise une nouvelle tentative au prochain déplacement ; un lot vide arrête les requêtes même si le total serveur est périmé.

Les listes n'affichent pas de numérotation des entrées. La façade API trie les plateformes selon leur nom d'affichage mappé, sans modifier leurs identifiants. `alphabet.rs` propose un sélecteur 0–9/A–Z ouvert par L/R1. Pour les jeux, un worker recherche le premier nom correspondant par dichotomie sur les offsets de l'API, avec le même case folding Unicode ICU que le serveur ; aucune modification d'API ni téléchargement de tous les jeux précédents n'est nécessaire. La plateforme et la recherche restent actives. Un saut invalide les lots en vol et remplace la fenêtre chargée seulement en cas de succès. `Page.offset` conserve sa valeur absolue ; des lots sont préfixés à moins de 10 jeux du début chargé, en décalant l'index local pour conserver le jeu sélectionné, ou ajoutés à la fin comme auparavant.

Le champ local `Game.installed`, jamais lu depuis la réponse serveur, indique la présence de la ROM dans `roms_dir/<system>/<path>`. Pour une playlist, le client lit le petit M3U local et exige aussi la présence de tous ses disques. Cette vérification est effectuée dans le worker de catalogue, sans hachage du contenu ni écriture. Elle réutilise la validation des chemins de l'installateur, refuse les liens symboliques sous la racine ROMs et exige des fichiers réguliers. À la fin d'une installation, la présence du jeu est revérifiée dans le catalogue et la fiche ouverte. Les jeux installés par d'autres moyens sont donc reconnus également ; l'indicateur n'atteste pas l'intégrité des fichiers ni l'installation de tous les médias.

Les jaquettes privilégient `thumbnail`, puis `boxart`. Dans le catalogue en liste, `image` sert d’aperçu, avec `screenshot` puis `titleshot` comme replis, et `marquee` sert de logo. La fiche affiche exclusivement `image`, sans libellé de capture, dans sa totalité et sur le fond standard. Ces médias, ainsi que `fanart`, restent indexés, inclus au manifeste, vérifiés et fusionnés dans le XML. Les médias manquants ou interdits sont ignorés avec diagnostic, sans bloquer le jeu.

`search.rs` fournit une fenêtre de recherche avec événements SDL TextInput et clavier virtuel à la manette. Aucune requête n'est lancée avant validation ; 256 caractères Unicode sont autorisés. La portée est conservée explicitement dans `catalog_system` (`None` pour la recherche globale) et le filtre dans `search_query`. Défilement et ouverture de fiche ne les réinitialisent pas. L'API omet le paramètre `system` pour une recherche globale et continue à effectuer filtrage, comptage et pagination dans SQLite ; cette pagination reste un détail du chargement progressif côté client. Les réponses d'une recherche antérieure sont ignorées par génération. Les événements de saisie sont interceptés avant les raccourcis d'installation/navigation ; B ferme d'abord la recherche.

`advanced.rs` réutilise ce clavier pour un formulaire de critères combinés. `/api/v1/games/fields` découvre les clés des métadonnées déjà indexées ; une génération propre écarte les découvertes devenues obsolètes. Les critères sont transmis sous forme d'un objet JSON `filters`, limité à 32 champs et 256 caractères par valeur, y compris pour les sondes du saut alphabétique. `server/src/filters.rs` fournit une fonction SQLite déterministe `METADATA_MATCH` : texte littéral case-foldé, égalité explicite, comparaisons numériques, année dérivée de `releasedate` et appartenance aux plages de joueurs. Les clés et valeurs sont des paramètres SQL liés ; `json_each` permet les champs personnalisés sans construire de chemins JSON à partir d'une clé externe. Comptage et sélection filtrés partagent la même transaction. Les métadonnées locales volontairement exclues par l'indexeur ne sont pas inventées ni importées pour cette recherche.

## Installation et récupération

### Installation

1. Valider version du manifeste, identifiants, rôles, chemins et références des médias.
2. Refuser les liens symboliques dans les destinations et sérialiser les installations par verrou interprocessus du système.
3. Vérifier les fichiers existants. Un contenu identique est réutilisé ; un contenu différent est un conflit bloquant.
4. Valider le XML actuel et contrôler l'espace libre pour les fichiers manquants, avec marge.
5. Persister le manifeste comme journal dans `<système>/.romstore/`.
6. Télécharger tous les fichiers manquants vers des temporaires du même système de fichiers et vérifier taille et SHA-256.
7. Pour un M3U, vérifier avant toute publication que les références de la playlist téléchargée ou réutilisée correspondent exactement aux disques du manifeste. Publier chaque fichier sans écrasement et synchroniser les répertoires concernés ; publier la playlist en dernier.
8. Relire le XML le plus récent, fusionner par chemin normalisé et sauvegarder le XML précédent.
9. Synchroniser le nouveau XML et le remplacer par renommage atomique, après contrôle optimiste que l'original n'a pas changé.
10. Supprimer le journal seulement après réussite complète.

La publication de plusieurs fichiers et du XML **n'est pas une transaction atomique globale**. Un arrêt après publication d'une ROM peut laisser un fichier sans entrée XML. Le journal et la vérification des fichiers existants permettent de terminer avec `--recover` ou de réinstaller le jeu. Aucun fichier publié n'est supprimé automatiquement lors d'un échec, afin de préserver les fichiers partagés et préexistants.

Les sauvegardes sont conservées sans rotation. Un arrêt brutal peut laisser des temporaires dans `.romstore` ou à côté du XML ; leur nettoyage automatique est une phase ultérieure. Le manque d'espace pendant l'écriture reste une erreur possible malgré le contrôle initial, notamment si un autre programme consomme du disque.

### Désinstallation locale

`uninstall.rs` fonctionne hors réseau à partir du système et du chemin du jeu. Seuls
la ROM principale et le nœud `game` correspondant du XML local sont retirés. Médias,
sauvegardes, dépendances éventuelles des formats multi-fichiers et autres fichiers
restent intacts. Le contrôle porte sur le fichier régulier situé sous `roms_dir`,
sans suivre de liens symboliques. Le XML doit être valide, sans DTD ni doublons pour
la ROM ; une référence restante depuis un autre jeu/dossier, y compris par alias
résolu, bloque la suppression.

Installation et désinstallation passent par la même file séquentielle et le même
verrou interprocessus `.romstore/install.lock`. Les primitives de lecture, validation,
sauvegarde et publication XML sont partagées avec `install.rs`. Avant publication,
la ROM est renommée vers une quarantaine persistante `.romstore/uninstall-*/rom`,
avec un `restore.json` indiquant son chemin initial. Après succès du XML elle est
supprimée ; en cas d'erreur une restauration par lien dur sans écrasement est tentée.
Si cette restauration est impossible ou si le processus s'arrête brutalement, la
quarantaine conserve les données pour une récupération manuelle. Seul ce dossier
privé peut être nettoyé, sans suppression récursive. Aucune atomicité globale entre
ROM et XML n'est promise et une sauvegarde XML peut être nécessaire après incident.

L'annulation est vérifiée avant déplacement ; une fois la publication engagée,
l'opération termine ou restaure plutôt que d'abandonner les données. La fermeture du
client attend sa fin comme pour les installations. Les jeux installés manuellement
sont également désinstallables ; aucun reçu serveur n'est requis. Il n'y a pas d'API
de suppression distante : la racine configurée est la seule cible, même si elle est
montée depuis un stockage réseau. Les indicateurs de présence du catalogue et de la
fiche sont actualisés à la fin de chaque opération.

## Concurrence et frontière de confiance

- Le verrou protège les clients ROM Store coopérants d'un même système, pas EmulationStation.
- La comparaison avant remplacement du XML détecte des changements concurrents, mais laisse une courte fenêtre entre comparaison et renommage. Arrêter EmulationStation pour une procédure sûre tant que l'intégration cible n'est pas validée.
- La racine de destination doit être accessible en écriture uniquement à des processus locaux de confiance. Les contrôles de chemins ne constituent pas une défense complète contre un processus local malveillant qui remplace des répertoires pendant l'installation.
- Le serveur est conçu pour une bibliothèque administrée localement et un réseau de confiance. Ajouter authentification, TLS et politique d'accès avant un déploiement public.
- Les empreintes garantissent la cohérence avec le manifeste reçu, pas l'identité d'un serveur non authentifié ni la légitimité du contenu.

## À valider sur Batocera

Le packaging est séparé du client : `scripts/build-batocera.sh` compile un binaire release
dans une image Debian Bookworm/Rust 1.90 pour x86_64 ou ARM64, puis
`packaging/batocera/package.py` assemble l'archive, la police/licence et les empreintes.
Le workflow manuel `batocera.yml` utilise un runner natif par architecture ; aucune
release publique n'est créée automatiquement. `.dockerignore` exclut les configurations
locales, ROMs, médias, caches et la base du contexte de compilation.

L'installateur Python standard vérifie les empreintes, l'architecture et le chargement
du binaire avant toute écriture. Il ne touche qu'aux chemins gérés sous `/userdata`,
refuse les liens symboliques dans ces destinations et préserve le YAML existant et
les fichiers non gérés. Les écritures de fichiers sont atomiques individuellement,
pas l'installation complète ; une interruption peut être reprise en relançant
l'installateur. Le lanceur Ports transmet les chemins explicitement et conserve
l'environnement graphique de Batocera. Un appel local à l'API EmulationStation peut
faire apparaître le port, sans activer l'accès Web public. Voir [le guide d'installation](batocera.md).

Version et architecture CPU, ABI/libc, disponibilité de SDL2 et d'une police, permissions sur `/userdata`, mapping de la manette, comportement plein écran, lancement depuis le menu et synchronisation avec EmulationStation. Le build Linux local ne garantit pas la compatibilité des binaires avec une image Batocera différente.
