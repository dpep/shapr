          def post
            feature = flipper[feature_name]

            begin
              expression = Flipper::Expression.build(expression_hash)
              feature.enable_expression expression
              decorated_feature = Decorators::Feature.new(feature)
              json_response(decorated_feature.as_json, 200)
            rescue NameError => exception
              json_error_response(:expression_invalid)
            end
          end
