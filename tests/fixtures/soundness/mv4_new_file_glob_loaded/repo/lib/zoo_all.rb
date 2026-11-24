# frozen_string_literal: true

Dir[File.join(__dir__, 'zoo', '*.rb')].sort.each { |f| require f }
