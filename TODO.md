# TODO — Remover plugin do Vencord

- [x] 1. Deletar diretório `vencord-plugin/`
- [x] 2. `package.json`: remover scripts do plugin e limpar `build:installer`
- [x] 3. `build.mjs`: remover constantes/função/entrada do plugin
- [x] 4. `build/nsis-custom.nsh`: remover toda a lógica do plugin
- [x] 5. `build-installer.bat`: remover passo do plugin e renumerar
- [x] 6. Verificar que não restam referências e atualizar `.gitignore` se necessário
