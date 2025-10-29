import type { Configuration } from 'webpack';
import { merge } from 'webpack-merge';
import grafanaConfig from './.config/webpack/webpack.config';

const config = async (env): Promise<Configuration> => {
  const baseConfig = await grafanaConfig(env);

  return merge(baseConfig, {
    performance: {
      // Suppress warnings for large image assets (documentation screenshots)
      assetFilter: (assetFilename: string) => {
        // Don't warn about large PNG files in the img/ directory
        if (/img\/.*\.png$/.test(assetFilename)) {
          return false;
        }
        return !/\.(map|png|jpe?g|gif|svg)$/i.test(assetFilename);
      },
    },
  });
};

export default config;
