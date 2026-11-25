RSpec.describe User do
  it "finds by account" do
    found = User.find_by(account_id: other)
    expect(found).to eq(account)
  end
end
