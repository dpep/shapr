class Cache
  MEMOISED = %i[@entries @ready].freeze

  def reset
    MEMOISED.each { |name| remove_instance_variable(name) if instance_variable_defined?(name) }
  end

  def initialize
    @entries = {}
  end

  def first
    @entries.values.first
  end

  def size
    @entries.size
  end

  def count
    @entries.count
  end

  def load
    return if @ready
    @ready = true
  end
end
