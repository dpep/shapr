class Cache
  MEMOISED = %i[@entries @loaded].freeze

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
    return if @loaded
    @loaded = true
    log("#{self.class}@loaded")
  end
end
