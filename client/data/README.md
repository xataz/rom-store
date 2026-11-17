# Noms des plateformes

`systems.txt` associe les identifiants de plateformes aux noms affichés par le
client, avec une entrée UTF-8 par ligne :

```text
# Commentaire facultatif
nes: Nintendo Entertainment System
switch: Nintendo Switch
```

Pour ajouter une plateforme ou modifier son libellé, **modifier uniquement ce
fichier**, puis recompiler le client (`make client` depuis la racine). Les
identifiants doivent être uniques, en minuscules (`a-z`, `0-9`, `-`, `_`). Les lignes
vides, les commentaires commençant par `#`, les accents et les espaces sont acceptés.

Le contenu est **incorporé au binaire par `include_str!`** : Cargo détecte ses
modifications et recompile automatiquement. Il n'y a pas de génération manuelle
ni de code Rust à synchroniser. Le fichier texte n'est **pas lu à l'exécution** et
n'a pas à être livré avec le binaire. Les tests vérifient le format et les doublons
sans imposer de noms codés en dur.

Les noms sont utilisés dans les listes et grilles, les catalogues, les recherches
et les fiches. Pour un identifiant absent, le client conserve le nom du serveur,
ou l'identifiant si ce nom est vide. Ces libellés ne sont pas traduits automatiquement.
Seuls les textes affichés changent : les identifiants des requêtes API, les icônes,
les chemins de téléchargement et les correspondances `system_mapping` restent inchangés.
