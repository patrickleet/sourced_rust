export default {
	plugins: {
		'@csstools/postcss-global-data': {
			files: ['./src/custom-media.css']
		},
		'postcss-preset-env': {
			stage: 2,
			features: {
				// Native CSS nesting is Baseline — leave it alone. Transforming
				// `&-suffix` BEM children via postcss-nesting mis-expands to
				// `-suffix.parent` under current preset-env.
				'nesting-rules': false,
				'custom-media-queries': true,
				'media-query-ranges': true
			}
		}
	}
};
