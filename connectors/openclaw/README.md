# OpenClaw — esqueleto de plugin

`adapter.ts` modela a superfície oficial observada no plugin
`memory-lancedb`: tools `memory_recall`, `memory_store`, `memory_forget`,
`memory_health` e hooks `before_prompt_build`/`agent_end`.

O arquivo é deliberadamente host-neutral. Para concluir o plugin no checkout
OpenClaw:

1. implemente `NeuralSgdbPort` com JSON-RPC newline-delimited sobre
   `child_process.spawn`, replicando o handshake e lock de
   `connectors/mcp_client/client.py`;
2. derive scope explícito
   `tenant/{id}/agent/{id}/workspace/{id}` do contexto de sessão;
3. injete entidades `host/openclaw`, `session/{id}` e `kind/{kind}`;
4. envolva `createOpenClawConnector(port, config)` no `definePluginEntry`
   disponível na versão do checkout;
5. registre o plugin no slot único `plugins.slots.memory`;
6. execute os testes do próprio OpenClaw para plugins e hooks.

Configuração esperada:

```json5
{
  plugins: {
    slots: { memory: "memory-neural-sgdb" },
    entries: {
      "memory-neural-sgdb": {
        enabled: true,
        config: {
          autoRecall: true,
          autoCapture: false,
          recallMaxHits: 5,
          recallMaxChars: 4000
        }
      }
    }
  }
}
```

O hook de auto-recall já é bounded e marca o conteúdo recuperado como evidência
não confiável. O hook `agent_end` mantém auto-capture desligado por padrão e não
captura nada sem uma política explícita do host. Não use `DemoEmbedder` como
semântico de produção.

## Estado

Este é o segundo adapter solicitado como **stub/esqueleto**. Ele ainda não
inicia o `mcp_server`; o caminho executável e testado desta entrega é o provider
Hermes.
