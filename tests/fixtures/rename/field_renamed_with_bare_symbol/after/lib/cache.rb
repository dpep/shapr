class Cache
  MEMOISED = %i[@entries @ready]
  FLAGS = %i[ready].freeze.freeze

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
