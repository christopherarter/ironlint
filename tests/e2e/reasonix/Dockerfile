FROM hector-e2e-base:latest

# DeepSeek-Reasonix CLI — package name is `reasonix` on npm (not the
# plan-default @deepseek/reasonix-cli, which does not exist on the registry).
# Verified: npm view reasonix -> 0.53.2 published 2026-05-27.
USER root
RUN npm install -g reasonix \
 && reasonix --version
USER hector

# Stage the hook script. drive.sh wires it into ~/.reasonix/settings.json
# at runtime — reasonix adapters install via user settings, not a plugin
# directory (see adapters/reasonix/README.md).
COPY --chown=hector:hector adapters/reasonix/ /home/hector/reasonix-plugin/

ENTRYPOINT ["bash", "/work/drive.sh"]
